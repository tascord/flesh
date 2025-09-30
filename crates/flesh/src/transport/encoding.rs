use {
    crate::transport::status::Status,
    chacha20poly1305::{
        ChaCha20Poly1305,
        aead::{Aead, KeyInit},
    },
    ed25519_dalek::{Signature, SigningKey, VerifyingKey, ed25519::signature::Signer},
    postcard,
    rand_core::{OsRng, RngCore},
    serde::{Deserialize, Serialize},
    std::{
        collections::HashMap,
        fmt::{Debug, Display},
        time::SystemTime,
    },
    thiserror::Error,
    uuid::Uuid,
    x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey, StaticSecret},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct FLESHMessage {
    pub version: u16,
    pub target: Option<Uuid>,
    pub sender: Option<Uuid>,
    pub timestamp: u64,
    pub headers: HashMap<String, Vec<u8>>,
    pub body: Vec<u8>,
    pub signature: Option<Vec<u8>>,
    pub status: Status,
}

impl FLESHMessage {
    pub fn new(status: Status) -> Self {
        Self {
            status,
            version: env!("CARGO_PKG_VERSION").split_once('.').unwrap().0.parse().unwrap(),
            target: None,
            sender: None,
            timestamp: SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap().as_secs(),
            headers: HashMap::new(),
            body: Vec::new(),
            signature: None,
        }
    }

    pub fn with_target(mut self, target: Uuid) -> Self {
        self.target = Some(target);
        self
    }

    pub fn with_sender(mut self, sender: Uuid) -> Self {
        self.sender = Some(sender);
        self
    }

    pub fn with_header(mut self, key: impl Display, value: impl Into<Vec<u8>>) -> Self {
        self.headers.insert(key.to_string(), value.into());
        self
    }

    pub fn with_body(mut self, body: impl Into<Vec<u8>>) -> Self {
        self.body = body.into();
        self
    }

    pub fn serialize(&self) -> Result<Vec<u8>, MessageError> {
        postcard::to_allocvec(self).map_err(MessageError::SerializationError)
    }

    pub fn deserialize(data: &[u8]) -> Result<Self, MessageError> {
        postcard::from_bytes(data).map_err(MessageError::DeserializationError)
    }

    pub fn sign(mut self, identity: impl Identity) -> anyhow::Result<Self> {
        self.sender = Some(identity.id());
        self.signature = None;

        let unsigned_data = self.serialize()?;
        let signature = identity.key().try_sign(&unsigned_data)?;
        self.signature = Some(signature.to_bytes().to_vec());

        Ok(self)
    }

    pub fn verify(&self, key: &VerifyingKey) -> Result<(), MessageError> {
        let signature_bytes = self.signature.as_ref().ok_or(MessageError::MissingSignature)?;

        let mut unsigned_msg = self.clone();
        unsigned_msg.signature = None;
        let unsigned_data = unsigned_msg.serialize()?;

        let signature =
            Signature::from_bytes(signature_bytes.as_slice().try_into().map_err(|_| MessageError::InvalidSignature)?);

        key.verify_strict(&unsigned_data, &signature).map_err(|_| MessageError::InvalidSignature)?;

        Ok(())
    }

    pub fn encrypt_body(mut self, target_key: &VerifyingKey) -> Result<Self, MessageError> {
        if self.body.is_empty() {
            return Ok(self);
        }

        let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
        let ephemeral_public = X25519PublicKey::from(&ephemeral_secret);

        let target_x25519 = X25519PublicKey::from(*target_key.as_bytes());
        let shared_secret = ephemeral_secret.diffie_hellman(&target_x25519);

        let cipher =
            ChaCha20Poly1305::new_from_slice(shared_secret.as_bytes()).map_err(|_| MessageError::EncryptionError)?;

        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = chacha20poly1305::Nonce::from_slice(&nonce_bytes);

        let encrypted = cipher.encrypt(nonce, self.body.as_ref()).map_err(|_| MessageError::EncryptionError)?;

        self.body = encrypted;
        self.headers.insert("ephemeral_key".to_string(), ephemeral_public.to_bytes().to_vec());
        self.headers.insert("nonce".to_string(), nonce.to_vec());

        Ok(self)
    }

    pub fn decrypt_body(mut self, identity: &impl Identity) -> Result<Self, MessageError> {
        let ephemeral_key = self.headers.get("ephemeral_key").ok_or(MessageError::MissingEncryptionData)?;
        let nonce_bytes = self.headers.get("nonce").ok_or(MessageError::MissingEncryptionData)?;

        let ephemeral_key: [u8; 32] =
            ephemeral_key.as_slice().try_into().map_err(|_| MessageError::InvalidEncryptionData)?;
        let ephemeral_public = X25519PublicKey::from(ephemeral_key);

        let my_secret = StaticSecret::from(identity.key().to_bytes());
        let shared_secret = my_secret.diffie_hellman(&ephemeral_public);

        let cipher =
            ChaCha20Poly1305::new_from_slice(shared_secret.as_bytes()).map_err(|_| MessageError::DecryptionError)?;
        let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);

        let decrypted = cipher.decrypt(nonce, self.body.as_ref()).map_err(|_| MessageError::DecryptionError)?;

        self.body = decrypted;
        self.headers.remove("ephemeral_key");
        self.headers.remove("nonce");

        Ok(self)
    }

    pub fn is_ok(&self) -> bool { self.status.is_ok() }

    /// If the target is broadcast, or targets the given identity
    pub fn for_id(&self, id: impl Identity) -> bool {
        self.target == Some(id.id()) || self.target == None
    }

}

#[derive(Debug, Error)]
pub enum MessageError {
    #[error("Serialization failed: {0}")]
    SerializationError(postcard::Error),
    #[error("Deserialization failed: {0}")]
    DeserializationError(postcard::Error),
    #[error("Missing signature")]
    MissingSignature,
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Encryption failed")]
    EncryptionError,
    #[error("Decryption failed")]
    DecryptionError,
    #[error("Missing encryption data")]
    MissingEncryptionData,
    #[error("Invalid encryption data")]
    InvalidEncryptionData,
}

pub trait Identity {
    fn id(&self) -> Uuid;
    fn key(&self) -> &SigningKey;
}

impl Identity for (Uuid, SigningKey) {
    fn id(&self) -> Uuid { self.0 }

    fn key(&self) -> &SigningKey { &self.1 }
}

impl Serialize for Status {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        u8::serialize(&self.as_u8(), serializer)
    }
}

impl<'de> Deserialize<'de> for Status {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let int = u8::deserialize(deserializer)?;
        Ok(Status::STANDARD.into_iter().find(|v| v.as_u8() == int).unwrap_or(Status::Custom(int)))
    }
}
