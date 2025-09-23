use {
    crate::transport::Transport,
    base64::{Engine, engine::general_purpose},
    chacha20poly1305::{
        ChaCha20Poly1305,
        aead::{Aead, KeyInit},
    },
    ed25519_dalek::{Signature, SigningKey, VerifyingKey, ed25519::signature::SignerMut},
    fl_uid::Fluid,
    rand_core::{OsRng, RngCore},
    std::{collections::HashMap, fmt::Debug},
    thiserror::Error,
    x25519_dalek::{EphemeralSecret, PublicKey as X25519PublicKey, StaticSecret},
};

#[derive(Clone)]
pub struct Message {
    version: u16,
    status: u16,
    target: Option<Fluid>,
    headers: HashMap<String, Vec<u8>>,
    body: Vec<u8>,
}

impl Debug for Message {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Message")
            .field("version", &self.version)
            .field("status", &self.status)
            .field("target", &self.target)
            .field("headers", &self.headers)
            .field("body", &format!("<{}b>", self.body.len()))
            .finish()
    }
}

impl Message {
    pub fn new() -> Self {
        Self {
            headers: Default::default(),
            body: Default::default(),
            status: Default::default(),
            version: env!("CARGO_PKG_VERSION").split_once('.').unwrap().0.parse().unwrap(),
            target: None,
        }
    }

    pub fn with_target(mut self, t: Fluid) -> Self {
        self.target = Some(t);
        self
    }

    pub fn with_status(mut self, s: u16) -> Self {
        self.status = s;
        self
    }

    pub fn with_header(mut self, k: String, v: Vec<u8>) -> Self {
        self.headers.insert(k.to_lowercase(), v);
        self
    }

    pub fn with_body(mut self, v: impl Into<Vec<u8>>) -> Self {
        self.body = v.into();
        self
    }

    pub fn serialize(&self) -> String {
        let mut out = format!(
            "{} {} {}\n",
            self.version,
            self.status,
            self.target.iter().map(|v| v.to_string()).collect::<Vec<_>>().join(",")
        );

        for (k, v) in &self.headers {
            let encoded_value = general_purpose::STANDARD.encode(v);
            out.push_str(&format!("{}:{}\n", k, encoded_value));
        }

        out.push('\n');
        out.push_str(&general_purpose::STANDARD.encode(&self.body));
        out
    }

    pub fn serialize_for_signing(&self) -> String {
        let mut temp_protocol = self.clone();
        temp_protocol.headers.remove("signature");
        temp_protocol.serialize()
    }

    pub fn signed(mut self, i: &impl Identity) -> String {
        let out = self.serialize();
        self = self.with_header("signature".to_owned(), i.key().clone().sign(out.as_bytes()).to_vec());
        self.serialize()
    }

    pub fn ok(&self) -> bool { self.status == 0 }

    pub fn headers(&self) -> &HashMap<String, Vec<u8>> { &self.headers }

    pub fn body(&self) -> &Vec<u8> { &self.body }

    pub fn encrypt_body(mut self, target_pub_key: &VerifyingKey) -> Result<Self, MessageDecodeError> {
        // Ensure a target is set
        if self.target.is_none() {
            return Err(MessageDecodeError::NoTargetProvided);
        }

        // Generate ephemeral keypair for Diffie-Hellman
        let ephemeral_secret = EphemeralSecret::random_from_rng(OsRng);
        let ephemeral_public = X25519PublicKey::from(&ephemeral_secret);

        // Convert target's Ed25519 public key to X25519 public key
        let target_x25519_pub = X25519PublicKey::from(*target_pub_key.as_bytes());

        // Compute the shared secret
        let shared_secret = ephemeral_secret.diffie_hellman(&target_x25519_pub);
        let key = shared_secret.as_bytes();

        // Encrypt the body using ChaCha20Poly1305
        let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|_| MessageDecodeError::InvalidKey)?;

        // **IMPORTANT:** Use a unique nonce for every message!
        let mut nonce_bytes = [0u8; 12];
        OsRng.fill_bytes(&mut nonce_bytes);
        let nonce = chacha20poly1305::Nonce::from_slice(&nonce_bytes);

        let encrypted_body = cipher.encrypt(nonce, self.body.as_ref()).map_err(|_| MessageDecodeError::EncryptionFailed)?;

        // Update message headers with the ephemeral key and nonce
        self.body = encrypted_body;
        self.headers.insert("ephemeral-key".to_owned(), ephemeral_public.to_bytes().to_vec());
        self.headers.insert("nonce".to_owned(), nonce.to_vec());

        Ok(self)
    }

    pub fn decrypt_body(mut self, i: &impl Identity) -> Result<Self, MessageDecodeError> {
        // Retrieve the ephemeral key and nonce from headers
        let ephemeral_key_bytes = self.headers.get("ephemeral-key").ok_or(MessageDecodeError::MissingEphemeralKey)?;
        let nonce_bytes = self.headers.get("nonce").ok_or(MessageDecodeError::MissingNonce)?;

        // Convert bytes to array for X25519PublicKey::from
        let ephemeral_key_array: [u8; 32] =
            ephemeral_key_bytes.as_slice().try_into().map_err(|_| MessageDecodeError::InvalidFormat)?;
        let ephemeral_public = X25519PublicKey::from(ephemeral_key_array);

        // Convert our Ed25519 signing key to X25519 static secret
        // Note: This is a direct byte conversion, not cryptographically equivalent keys
        let my_secret_bytes: [u8; 32] = i.key().to_bytes();
        let my_x25519_secret = StaticSecret::from(my_secret_bytes);
        let shared_secret = my_x25519_secret.diffie_hellman(&ephemeral_public);
        let key = shared_secret.as_bytes();

        // Decrypt the body
        let cipher = ChaCha20Poly1305::new_from_slice(key).map_err(|_| MessageDecodeError::InvalidKey)?;
        let nonce = chacha20poly1305::Nonce::from_slice(nonce_bytes);

        let decrypted_body = cipher.decrypt(nonce, self.body.as_ref()).map_err(|_| MessageDecodeError::DecryptionFailed)?;

        self.body = decrypted_body;
        Ok(self)
    }

    pub fn verify(&self, key: &VerifyingKey) -> Result<(), MessageDecodeError> {
        let signature_b64 = self.headers.get("signature").ok_or(MessageDecodeError::InvalidSignature)?;
        let signature_bytes = general_purpose::STANDARD.decode(signature_b64)?;

        let signature = Signature::from_bytes(&signature_bytes.try_into().map_err(|_| MessageDecodeError::InvalidFormat)?);
        let message = self.serialize_for_signing();

        key.verify_strict(message.as_bytes(), &signature).map_err(|_| MessageDecodeError::InvalidSignature)?;
        Ok(())
    }

    pub fn deserialize(s: &str, me: &impl Identity) -> Result<Self, MessageDecodeError> {
        let mut lines = s.lines();

        let first_line = lines.next().ok_or(MessageDecodeError::InvalidFormat)?;
        let mut parts = first_line.splitn(3, ' ');
        let version_str = parts.next().ok_or(MessageDecodeError::InvalidFormat)?;
        let status_str = parts.next().ok_or(MessageDecodeError::InvalidFormat)?;

        let target = parts.find(|p| !p.is_empty());
        if let Some(target) = target
            && target != me.id().to_string()
        {
            return Err(MessageDecodeError::NotRecipient);
        }

        let version = version_str.parse::<u16>().map_err(|_| MessageDecodeError::InvalidVersion(version_str.to_string()))?;
        let status = status_str.parse::<u16>().map_err(|_| MessageDecodeError::InvalidStatus(status_str.to_string()))?;

        let mut headers = HashMap::new();
        let mut body_lines = Vec::new();

        let mut in_headers = true;
        for line in lines {
            if in_headers {
                if line.is_empty() {
                    in_headers = false;
                    continue;
                }
                let mut header_parts = line.splitn(2, ':');
                let key = header_parts.next().ok_or(MessageDecodeError::InvalidFormat)?.to_string();
                let value_b64 = header_parts.next().ok_or(MessageDecodeError::InvalidFormat)?;
                let value = general_purpose::STANDARD.decode(value_b64)?;
                headers.insert(key, value);
            } else {
                body_lines.push(line);
            }
        }

        let body_b64 = body_lines.join("");
        let body = general_purpose::STANDARD.decode(body_b64)?;

        let deser = Self { version, status, headers, body, target: target.map(|_| me.id()) };

        if target.is_some() {
            deser.verify(&me.key().verifying_key())?;
            deser.decrypt_body(me)
        } else {
            Ok(deser)
        }
    }
}

impl Default for Message {
    fn default() -> Self { Self::new() }
}

#[derive(Debug, Error)]
pub enum MessageDecodeError {
    #[error("Invalid protocol format")]
    InvalidFormat,
    #[error("Invalid version: {0}")]
    InvalidVersion(String),
    #[error("Invalid status: {0}")]
    InvalidStatus(String),
    #[error("Invalid base64 encoding")]
    InvalidBase64(#[from] base64::DecodeError),
    #[error("Invalid signature")]
    InvalidSignature,
    #[error("Not recipient")]
    NotRecipient,
    #[error("No target fluid provided for encryption")]
    NoTargetProvided,
    #[error("Encryption failed")]
    EncryptionFailed,
    #[error("Decryption failed")]
    DecryptionFailed,
    #[error("Invalid key for cipher")]
    InvalidKey,
    #[error("Missing ephemeral key header")]
    MissingEphemeralKey,
    #[error("Missing nonce header")]
    MissingNonce,
}

pub trait Identity {
    fn id(&self) -> Fluid;
    fn key(&self) -> &SigningKey;
}

impl Identity for (Fluid, SigningKey) {
    fn id(&self) -> Fluid { self.0 }

    fn key(&self) -> &SigningKey { &self.1 }
}

impl<T> Identity for T
where
    T: Transport,
{
    fn id(&self) -> Fluid { self.resolver().id }

    fn key(&self) -> &SigningKey { &self.resolver().key }
}
