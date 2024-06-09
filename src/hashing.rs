use serde::de::{Error, Expected, Unexpected};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use sha1::{Digest, Sha1};
use sha2::{Sha256, Sha512};
use std::any::Any;
use std::fmt::{Display, Formatter};
use std::io::Write;

#[derive(Debug)]
pub struct HexString<const N: usize> {
    pub inner: [u8; N],
}

pub type Sha1String = HexString<20>;
pub type Sha2String = HexString<32>;
pub type Sha512String = HexString<64>;

fn parse_hex_string<'de, D>(str: &str, result: &mut [u8]) -> Result<(), D::Error>
where
    D: Deserializer<'de>,
{
    fn digit_value<'de, D>(char: u8) -> Result<u8, D::Error>
    where
        D: Deserializer<'de>,
    {
        (char as char)
            .to_digit(16)
            .map(|d| d as u8)
            .ok_or_else(|| Error::invalid_value(Unexpected::Char(char as char), &"hex string"))
    }

    for (i, chunk) in str.as_bytes().chunks_exact(2).enumerate() {
        result[i] = 16 * digit_value::<D>(chunk[0])? + digit_value::<D>(chunk[1])?;
    }

    Ok(())
}

fn to_hex_string(array: &[u8]) -> String {
    let mut str = String::with_capacity(array.len() * 2);
    for &value in array {
        str.push(char::from_digit((value >> 4) as u32, 16).unwrap());
        str.push(char::from_digit((value & 15) as u32, 16).unwrap());
    }
    str
}

impl<'de, const N: usize> Deserialize<'de> for HexString<N> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let str: String = Deserialize::deserialize(deserializer)?;
        if str.len() != N * 2 {
            struct ExpectedSize(usize);
            impl Expected for ExpectedSize {
                fn fmt(&self, formatter: &mut Formatter) -> std::fmt::Result {
                    write!(formatter, "hex string of length {}", self.0)
                }
            }

            return Err(Error::invalid_length(str.len(), &ExpectedSize(N * 2)));
        }

        let mut result = [0; N];
        parse_hex_string::<D>(&str, &mut result)?;
        Ok(HexString { inner: result })
    }
}

impl<const N: usize> Serialize for HexString<N> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        to_hex_string(&self.inner).serialize(serializer)
    }
}

#[derive(Debug, Copy, Clone, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HashAlgorithm {
    Sha1,
    Sha256,
    Sha512,
}

impl HashAlgorithm {
    pub fn create_hasher(&self) -> Box<dyn DigestHasher> {
        match self {
            Self::Sha1 => Box::new(Sha1::new()),
            Self::Sha256 => Box::new(Sha256::new()),
            Self::Sha512 => Box::new(Sha512::new()),
        }
    }

    pub fn hash_size(&self) -> usize {
        match self {
            Self::Sha1 => 20,
            Self::Sha256 => 32,
            Self::Sha512 => 64,
        }
    }
}

impl Display for HashAlgorithm {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Sha1 => f.write_str("sha1"),
            Self::Sha256 => f.write_str("sha256"),
            Self::Sha512 => f.write_str("sha512"),
        }
    }
}

pub trait DigestHasher: Write + Any {
    fn finalize(self: Box<Self>) -> Box<[u8]>;
}

impl<T> DigestHasher for T
where
    T: Write + Digest + Any,
{
    fn finalize(self: Box<Self>) -> Box<[u8]> {
        Digest::finalize(*self).to_vec().into_boxed_slice()
    }
}

#[derive(Debug)]
pub struct HashWithAlgorithm {
    pub algorithm: HashAlgorithm,
    pub hash: Box<[u8]>,
}

impl<'de> Deserialize<'de> for HashWithAlgorithm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let proxy = HashWithAlgorithmSerdeProxy::deserialize(deserializer)?;
        if proxy.hash.len() != proxy.algorithm.hash_size() * 2 {
            struct ExpectedSize(HashAlgorithm);
            impl Expected for ExpectedSize {
                fn fmt(&self, formatter: &mut Formatter) -> std::fmt::Result {
                    write!(
                        formatter,
                        "hex string of length {} for {}",
                        self.0.hash_size() * 2,
                        self.0
                    )
                }
            }

            return Err(Error::invalid_length(
                proxy.hash.len(),
                &ExpectedSize(proxy.algorithm),
            ));
        }

        let mut hash = vec![0; proxy.algorithm.hash_size()].into_boxed_slice();
        parse_hex_string::<D>(&proxy.hash, &mut hash)?;
        Ok(HashWithAlgorithm {
            algorithm: proxy.algorithm,
            hash,
        })
    }
}

impl Serialize for HashWithAlgorithm {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let proxy = HashWithAlgorithmSerdeProxy {
            algorithm: self.algorithm,
            hash: to_hex_string(&self.hash),
        };
        proxy.serialize(serializer)
    }
}

#[derive(Deserialize, Serialize)]
struct HashWithAlgorithmSerdeProxy {
    algorithm: HashAlgorithm,
    hash: String,
}
