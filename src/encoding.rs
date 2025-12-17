use anyhow::{Result, anyhow};
use bytes::Buf;
use std::collections::BTreeMap;
use tokio_util::bytes::BytesMut;
use tokio_util::codec::{Decoder as TokioDecoder, Encoder as TokioEncoder};

use crate::error::CodecError;
use crate::messages::PeerMessage;
use crate::types::BencodeTypes;

#[derive(Debug, PartialEq, Eq)]
pub struct Encoder {}

impl Encoder {
    #[allow(clippy::wrong_self_convention)]
    pub fn from_bencode_types(&self, input: BencodeTypes) -> Result<Vec<u8>> {
        match input {
            BencodeTypes::String(s) => {
                let res = self.from_string(s)?;
                Ok(res)
            }
            BencodeTypes::Integer(i) => {
                let res = self.from_integer(i)?;
                Ok(res)
            }
            BencodeTypes::List(l) => {
                let res = self.from_list(l)?;
                Ok(res)
            }
            BencodeTypes::Dictionary(d) => {
                let res = self.from_dictionary(d)?;
                Ok(res)
            }
            BencodeTypes::Raw(r) => Ok(r),
        }
    }

    #[allow(clippy::wrong_self_convention)]
    fn from_string(&self, input: String) -> Result<Vec<u8>> {
        let output = format!("{}:{}", input.len(), input);
        Ok(output.as_bytes().to_vec())
    }

    #[allow(clippy::wrong_self_convention)]
    fn from_integer(&self, input: isize) -> Result<Vec<u8>> {
        let output = format!("i{}e", input);
        Ok(output.as_bytes().to_vec())
    }

    #[allow(clippy::wrong_self_convention)]
    fn from_list(&self, input: Vec<BencodeTypes>) -> Result<Vec<u8>> {
        let mut raw = Vec::new();
        raw.push(b'l');
        for item in input {
            let res = self.from_bencode_types(item)?;
            raw.extend(res);
        }
        raw.push(b'e');
        Ok(raw)
    }

    #[allow(clippy::wrong_self_convention)]
    fn from_dictionary(&self, input: BTreeMap<String, BencodeTypes>) -> Result<Vec<u8>> {
        let mut raw = Vec::new();
        raw.push(b'd');
        for (key, value) in input {
            let res = self.from_string(key.clone())?;
            raw.extend(res);

            let val = if key == "pieces" {
                self.encode_pieces(value)?
            } else {
                self.from_bencode_types(value)?
            };

            raw.extend(val);
        }

        raw.push(b'e');
        Ok(raw)
    }

    fn encode_pieces(&self, input: BencodeTypes) -> Result<Vec<u8>> {
        match input {
            BencodeTypes::List(l) => {
                let mut len = 0;
                let mut data = Vec::new();

                for item in l {
                    if let BencodeTypes::String(s) = item {
                        let value = hex::decode(s)?;
                        len += value.len();
                        data.extend(value);
                    } else {
                        return Err(anyhow!("the provided data is not a string"));
                    }
                }

                let prefix = format!("{}:", len).into_bytes();
                let mut res = Vec::with_capacity(prefix.len() + data.len());
                res.extend_from_slice(&prefix);
                res.extend_from_slice(&data);
                Ok(res)
            }
            BencodeTypes::Raw(s) => {
                let len = s.len();
                let prefix = format!("{}:", len).into_bytes();
                let mut res = Vec::with_capacity(prefix.len() + s.len());
                res.extend_from_slice(&prefix);
                res.extend_from_slice(&s);
                Ok(res)
            }

            _ => Err(anyhow!("the provided data is not a list or a string")),
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Decoder {}

impl Decoder {
    // Returns the number of bytes read and the decoded type
    #[allow(clippy::wrong_self_convention)]
    pub fn from_bytes(&self, bytes: &[u8]) -> Result<(usize, BencodeTypes)> {
        if self.is_list(bytes) {
            let (n, val) = self.decode_list(bytes)?;
            return Ok((n, BencodeTypes::List(val)));
        }

        if self.is_dictionary(bytes) {
            let (n, val) = self.decode_dictionary(bytes)?;
            return Ok((n, BencodeTypes::Dictionary(val)));
        }

        let (n, val) = self.decode_primitive(bytes)?;
        Ok((n, val))
    }

    fn decode_primitive(&self, bytes: &[u8]) -> Result<(usize, BencodeTypes)> {
        // The order of the checks is important or it will misconsider the data type.
        if self.is_dictionary(bytes) {
            let (n, val) = self.decode_dictionary(bytes)?;
            return Ok((n, BencodeTypes::Dictionary(val)));
        }

        if self.is_list(bytes) {
            let (n, val) = self.decode_list(bytes)?;
            return Ok((n, BencodeTypes::List(val)));
        }

        if self.is_integer(bytes) {
            let (n, val) = self.decode_integer(bytes)?;
            return Ok((n, BencodeTypes::Integer(val)));
        }

        if self.is_string(bytes) {
            let (n, val) = self.decode_string(bytes)?;
            return Ok((n, BencodeTypes::String(val)));
        }

        Err(anyhow!("the provided data is not a valid primitive"))
    }

    fn get_next_data_len(&self, bytes: &[u8]) -> Result<(usize, usize)> {
        let mut curr_idx = 0;
        while curr_idx < bytes.len() {
            let b = bytes[curr_idx];
            if b == b':' {
                break;
            }

            // Only allow digits before colon
            if !b.is_ascii_digit() {
                return Err(anyhow!("the provided data is not a string"));
            }
            curr_idx += 1;
        }

        let next_data_len: &str = std::str::from_utf8(&bytes[0..curr_idx])?;
        let len: usize = next_data_len.parse()?;

        Ok((curr_idx, len))
    }

    fn decode_pieces_hashes(&self, bytes: &[u8]) -> Result<(usize, Vec<BencodeTypes>)> {
        let (n, len) = self.get_next_data_len(bytes)?;
        if len % 20 != 0 {
            return Err(anyhow!("the provided data is not a valid pieces hashes"));
        }

        let mut curr_idx = n + 1; // +1 because the curr_idx is in the colon
        let mut hashes = Vec::new();
        while curr_idx < len {
            let hash = &bytes[curr_idx..curr_idx + 20];
            hashes.push(BencodeTypes::String(hex::encode(hash)));
            curr_idx += 20;
        }

        Ok((curr_idx, hashes))
    }

    fn decode_string(&self, bytes: &[u8]) -> Result<(usize, String)> {
        if !self.is_string(bytes) {
            return Err(anyhow!("the provided data is not a string"));
        }

        let mut curr_idx = 0;
        loop {
            if bytes[curr_idx] as char == ':' {
                break;
            }
            curr_idx += 1;
        }
        let str_len = &bytes[0..curr_idx];
        let str_len_str: &str = std::str::from_utf8(str_len)?;
        let len: usize = str_len_str.parse()?;

        curr_idx += 1; // ignore the colon in the string
        let input = &bytes[curr_idx..len + curr_idx];
        let str = String::from_utf8_lossy(input).to_string();

        let n = len + curr_idx; // considers the len of the string in the message and the colon
        Ok((n, str.to_string()))
    }

    fn decode_integer(&self, bytes: &[u8]) -> Result<(usize, isize)> {
        if !self.is_integer(bytes) {
            return Err(anyhow!("the provided data is not a integer"));
        }

        let mut end = 0;
        loop {
            if bytes[end] as char == 'e' {
                break;
            }
            end += 1
        }

        let input = &bytes[1..end];
        let str = std::str::from_utf8(input).expect("Invalid UTF-8 string");
        let number = str.parse::<isize>()?;
        let n = 1 + end; // 1 because if the 'i'. The end already considers the 'e'.
        Ok((n, number))
    }

    fn decode_list(&self, bytes: &[u8]) -> Result<(usize, Vec<BencodeTypes>)> {
        if !self.is_list(bytes) {
            return Err(anyhow!("the provided data is not a list"));
        }

        let mut curr_idx = 0;
        curr_idx += 1; // ignore the 'l' in the provided bytes

        let mut result = Vec::new();

        while curr_idx < bytes.len() {
            if bytes[curr_idx] as char == 'e' {
                // reached the end of this list
                break;
            }

            let (n, val) = self.decode_primitive(&bytes[curr_idx..])?;
            result.push(val);
            curr_idx += n;
        }

        // +1 because of the 'e' in the provided bytes
        Ok((curr_idx + 1, result))
    }

    fn decode_dictionary(&self, bytes: &[u8]) -> Result<(usize, BTreeMap<String, BencodeTypes>)> {
        if !(self.is_dictionary(bytes)) {
            return Err(anyhow!("the provided data is not a dictionary"));
        }

        let input = &bytes[1..]; // the 'd' in the provided bytes

        let mut curr_idx = 0;
        let mut hm = BTreeMap::new();
        while curr_idx < input.len() {
            if input[curr_idx] as char == 'e' {
                // reached the end of the dictionary
                break;
            }

            let (n, key) = self.decode_string(&input[curr_idx..])?;
            curr_idx += n;

            // the pieces key has special caracters that are not valid UTF-8 characters, so we need to handle it as raw.
            let val: BencodeTypes = if key == "pieces" {
                // TODO: Consider if I want to still parse the pieces as raw or if the string format is good enough.
                // debug!("[decode_dictionary] pieces key found");
                // let (n, v) = self.handle_as_raw(&input[curr_idx..])?;
                // curr_idx += n;
                // v

                let (n, v) = self.decode_pieces_hashes(&input[curr_idx..])?;
                curr_idx += n;
                BencodeTypes::List(v)
            } else {
                let (n, v) = self.decode_primitive(&input[curr_idx..])?;
                curr_idx += n;
                v
            };

            hm.insert(key, val);
        }

        Ok((curr_idx + 2, hm)) // +2 because of the 'd' and the 'e' in the provided bytes
    }

    fn is_list(&self, bytes: &[u8]) -> bool {
        bytes.len() >= 2 && bytes[0] as char == 'l' && bytes[bytes.len() - 1] as char == 'e'
    }

    fn is_string(&self, bytes: &[u8]) -> bool {
        if self.get_next_data_len(bytes).is_ok() {
            let (n, _) = self.get_next_data_len(bytes).unwrap();
            n > 0 && n < bytes.len() && bytes[n] == b':'
        } else {
            false
        }
    }

    fn is_integer(&self, bytes: &[u8]) -> bool {
        let mut curr_idx = 0;
        while curr_idx < bytes.len() {
            if bytes[curr_idx] as char == 'e' {
                break;
            }
            curr_idx += 1;
        }

        bytes.len() >= 2 && bytes[0] as char == 'i' && curr_idx < bytes.len()
    }

    fn is_dictionary(&self, bytes: &[u8]) -> bool {
        bytes.len() >= 2 && bytes[0] as char == 'd' && bytes[bytes.len() - 1] as char == 'e'
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_decode_string() {
        let test_cases: Vec<(&'static str, Vec<u8>, Result<(usize, String)>)> = vec![
            (
                "01 - simple string",
                "5:hello".as_bytes().to_vec(),
                Ok((7, String::from("hello"))),
            ),
            (
                "02 - empty bytes",
                vec![],
                Err(anyhow!("the provided data is not a string")),
            ),
            (
                "03 - simple string",
                "4:rust".as_bytes().to_vec(),
                Ok((6, String::from("rust"))),
            ),
            (
                "04 - longer string with len greater than 10",
                "35:https://torrent.ubuntu.com/announce".as_bytes().to_vec(),
                Ok((38, String::from("https://torrent.ubuntu.com/announce"))),
            ),
        ];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let decoder = Decoder {};
            let res = decoder.decode_string(&input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(
                    res.unwrap_err()
                        .to_string()
                        .contains(&expected.unwrap_err().to_string())
                );
            } else {
                panic!("Result variants did not match");
            }
        }
    }

    #[test]
    fn test_decode_integer() {
        let test_cases = vec![
            (
                "01 - simple integer",
                "i42e".as_bytes().to_vec(),
                Ok((4, 42)),
            ),
            (
                "02 - empty bytes",
                vec![],
                Err(anyhow!("the provided data is not a integer")),
            ),
            (
                "03 - negative integer",
                "i-42e".as_bytes().to_vec(),
                Ok((5, -42)),
            ),
        ];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let decoder = Decoder {};
            let res = decoder.decode_integer(&input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(
                    res.unwrap_err()
                        .to_string()
                        .contains(&expected.unwrap_err().to_string())
                );
            } else {
                panic!("Result variants did not match");
            }
        }
    }

    #[test]
    fn test_decode_list() {
        let test_cases = vec![
            (
                "01 - simple list",
                "l4:rust6:golangi40ei-60ee".as_bytes().to_vec(),
                Ok::<_, anyhow::Error>((
                    25,
                    vec![
                        BencodeTypes::String(String::from("rust")),
                        BencodeTypes::String(String::from("golang")),
                        BencodeTypes::Integer(40),
                        BencodeTypes::Integer(-60),
                    ],
                )),
            ),
            ("02 - empty list", "le".as_bytes().to_vec(), Ok((2, vec![]))),
            (
                "03 - list with a single string",
                "l5:helloi52ee".as_bytes().to_vec(),
                Ok((
                    13,
                    vec![
                        BencodeTypes::String(String::from("hello")),
                        BencodeTypes::Integer(52),
                    ],
                )),
            ),
            (
                "04 - list with a single integer",
                "li40ee".as_bytes().to_vec(),
                Ok((6, vec![BencodeTypes::Integer(40)])),
            ),
            (
                "05 - list within a list",
                "ll5:helloel5:worldee".as_bytes().to_vec(),
                Ok((
                    20,
                    vec![
                        BencodeTypes::List(vec![BencodeTypes::String(String::from("hello"))]),
                        BencodeTypes::List(vec![BencodeTypes::String(String::from("world"))]),
                    ],
                )),
            ),
            ("06 - empty list", "le".as_bytes().to_vec(), Ok((2, vec![]))),
            (
                "07 - list with multiple dictionaries",
                "ld3:foo3:bared3:baz3:quxee".as_bytes().to_vec(),
                Ok::<_, anyhow::Error>((
                    26,
                    vec![
                        BencodeTypes::Dictionary(BTreeMap::from([(
                            String::from("foo"),
                            BencodeTypes::String(String::from("bar")),
                        )])),
                        BencodeTypes::Dictionary(BTreeMap::from([(
                            String::from("baz"),
                            BencodeTypes::String(String::from("qux")),
                        )])),
                    ],
                )),
            ),
        ];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let decoder = Decoder {};
            let res = decoder.decode_list(&input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(
                    res.unwrap_err()
                        .to_string()
                        .contains(&expected.unwrap_err().to_string())
                );
            } else {
                panic!("Result variants did not match");
            }
        }
    }

    #[test]
    fn test_decode_dictionary() {
        let test_cases = vec![(
            "01 - simple dictionary",
            "d3:foo3:bar5:helloi52ee".as_bytes().to_vec(),
            Ok::<_, anyhow::Error>((
                23,
                BTreeMap::from([
                    (
                        String::from("foo"),
                        BencodeTypes::String(String::from("bar")),
                    ),
                    (String::from("hello"), BencodeTypes::Integer(52)),
                ]),
            )),
        )];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let decoder = Decoder {};
            let res = decoder.decode_dictionary(&input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(
                    res.unwrap_err()
                        .to_string()
                        .contains(&expected.unwrap_err().to_string())
                );
            } else {
                panic!("Result variants did not match");
            }
        }
    }

    #[test]
    fn test_encode_bencode_string() {
        let test_cases = vec![(
            "01 - simple string",
            BencodeTypes::String(String::from("hello")),
            Ok::<_, anyhow::Error>(Vec::from("5:hello")),
        )];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let encoder = Encoder {};
            let res = encoder.from_bencode_types(input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(
                    res.unwrap_err()
                        .to_string()
                        .contains(&expected.unwrap_err().to_string())
                );
            } else {
                panic!("Result variants did not match");
            }
        }
    }

    #[test]
    fn test_encode_bencode_integer() {
        let test_cases = vec![
            (
                "01 - simple integer",
                BencodeTypes::Integer(42),
                Ok::<_, anyhow::Error>(Vec::from("i42e")),
            ),
            (
                "02 - negative integer",
                BencodeTypes::Integer(-42),
                Ok::<_, anyhow::Error>(Vec::from("i-42e")),
            ),
        ];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let encoder = Encoder {};
            let res = encoder.from_bencode_types(input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(
                    res.unwrap_err()
                        .to_string()
                        .contains(&expected.unwrap_err().to_string())
                );
            } else {
                panic!("Result variants did not match");
            }
        }
    }

    #[test]
    fn test_encode_bencode_list() {
        let test_cases = vec![
            (
                "01 - simple list",
                BencodeTypes::List(vec![
                    BencodeTypes::String(String::from("hello")),
                    BencodeTypes::Integer(42),
                ]),
                Ok::<_, anyhow::Error>(Vec::from("l5:helloi42ee")),
            ),
            (
                "02 - list within a list",
                BencodeTypes::List(vec![
                    BencodeTypes::String(String::from("hello")),
                    BencodeTypes::Integer(42),
                    BencodeTypes::List(vec![BencodeTypes::String(String::from("world"))]),
                ]),
                Ok::<_, anyhow::Error>(Vec::from("l5:helloi42el5:worldee")),
            ),
        ];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let encoder = Encoder {};
            let res = encoder.from_bencode_types(input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(
                    res.unwrap_err()
                        .to_string()
                        .contains(&expected.unwrap_err().to_string())
                );
            } else {
                panic!("Result variants did not match");
            }
        }
    }

    #[test]
    fn test_encode_bencode_dictionary() {
        let test_cases = vec![(
            "01 - simple dictionary",
            BencodeTypes::Dictionary(BTreeMap::from([(
                String::from("foo"),
                BencodeTypes::String(String::from("bar")),
            )])),
            Ok::<_, anyhow::Error>(Vec::from("d3:foo3:bare")),
        )];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let encoder = Encoder {};
            let res = encoder.from_bencode_types(input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(
                    res.unwrap_err()
                        .to_string()
                        .contains(&expected.unwrap_err().to_string())
                );
            } else {
                panic!("Result variants did not match");
            }
        }
    }
}

pub struct PeerMessageDecoder {
    num_pieces: usize,
}

impl PeerMessageDecoder {
    pub fn new(num_pieces: usize) -> Self {
        Self { num_pieces }
    }
}

impl TokioDecoder for PeerMessageDecoder {
    type Item = PeerMessage;
    type Error = anyhow::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        match PeerMessage::from_bytes(src.as_ref(), self.num_pieces) {
            Ok((n, message)) => {
                src.advance(n);
                Ok(Some(message))
            }
            Err(e) => {
                if let Some(codec_err) = e.downcast_ref::<CodecError>() {
                    match codec_err {
                        CodecError::IncompleteMessage { .. } | CodecError::MessageTooShort(_) => {
                            Ok(None)
                        }
                        _ => Err(e),
                    }
                } else {
                    Err(e)
                }
            }
        }
    }
}

pub struct PeerMessageEncoder {
    #[allow(dead_code)]
    num_pieces: usize,
}

impl PeerMessageEncoder {
    pub fn new(num_pieces: usize) -> Self {
        Self { num_pieces }
    }
}

impl TokioEncoder<PeerMessage> for PeerMessageEncoder {
    type Error = anyhow::Error;

    fn encode(&mut self, item: PeerMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes = item.to_bytes();
        if let Err(e) = bytes {
            return Err(anyhow!("error converting message to bytes: {}", e));
        }

        dst.extend_from_slice(&bytes.unwrap());
        Ok(())
    }
}
