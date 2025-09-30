// use {
//     crate::transport::Transport,
//     serde::{Deserialize, Serialize},
//     std::{
//         collections::HashMap,
//         hash::Hash,
//         io::{self, Cursor},
//         marker::PhantomData,
//     },
// };

// #[derive(Serialize, Deserialize, Debug)]
// pub enum DhtMessage<K, V> {
//     GetRequest(K),
//     GetResponse(Option<V>),
//     PutRequest(K, V),
// }

// // The core DHT implementation
// pub struct MeshTable<K, V, T: Transport, F1, F2>
// where
//     K: Hash + Eq,
//     F1: Fn(&V) -> io::Result<Vec<u8>>,
//     F2: Fn(&[u8]) -> io::Result<V>,
// {
//     local: HashMap<K, V>,
//     transport: T,
//     encoder: F1,
//     decoder: F2,
//     __marker: PhantomData<(K, V)>,
// }

// impl<K, V, T: Transport, F1, F2> MeshTable<K, V, T, F1, F2>
// where
//     K: Hash + Eq,
//     F1: Fn(&V) -> io::Result<Vec<u8>>,
//     F2: Fn(&[u8]) -> io::Result<V>,
// {
//     pub fn new(transport: T, enc: F1, dec: F2) -> Self {
//         Self { local: HashMap::new(), transport, __marker: PhantomData, encoder: enc, decoder: dec }
//     }

//     pub fn insert_local(&mut self, k: K, v: V) { self.local.insert(k, v); }
// }

// pub mod compression {
//     use super::*;

//     // SJ/LZMA ---------------

//     fn ser_sj_lzlma<V>(v: &V) -> std::io::Result<Vec<u8>>
//     where
//         V: Serialize,
//     {
//         let text = serde_json::to_string(v)
//             .map(|v| v.as_bytes().to_vec())
//             .map_err(|e| std::io::Error::new(io::ErrorKind::InvalidData, e))?;

//         let mut compressed: Vec<u8> = Vec::new();
//         lzma_rs::lzma2_compress(&mut Cursor::new(text), &mut compressed)?;
//         Ok(compressed)
//     }

//     fn deser_sj_lzlma<V>(v: &[u8]) -> std::io::Result<V>
//     where
//         V: for<'de> Deserialize<'de>,
//     {
//         let mut decomp: Vec<u8> = Vec::new();
//         lzma_rs::lzma2_decompress(&mut Cursor::new(v), &mut decomp)
//             .map_err(|e| std::io::Error::new(io::ErrorKind::InvalidData, e))?;

//         serde_json::from_slice(&decomp).map_err(|e| std::io::Error::new(io::ErrorKind::InvalidData, e))
//     }

//     pub type SjLzmaMeshTable<K, V, T> = MeshTable<K, V, T, fn(&V) -> io::Result<Vec<u8>>, fn(&[u8]) -> io::Result<V>>;

//     impl<K, V, T: Transport> SjLzmaMeshTable<K, V, T>
//     where
//         K: Hash + Eq + Serialize + for<'de> Deserialize<'de> + Clone,
//         V: Serialize + for<'de> Deserialize<'de> + Clone,
//     {
//         pub fn new_sjlzma_typed(transport: T) -> Self {
//             Self { transport, local: HashMap::new(), encoder: ser_sj_lzlma, decoder: deser_sj_lzlma, __marker: PhantomData }
//         }
//     }
// }
