use std::{collections::HashMap, fs};
use anyhow::{anyhow, Ok, Result};
use log::debug;

fn main() {
    log::set_max_level(log::LevelFilter::Debug);
    
    let decoder = Decoder {};
    let mut torrent = BitTorrent::new(decoder);
    torrent.load_file("./tests/testdata/ubuntu-24.04.3-desktop-amd64.iso.torrent").unwrap();
    
    debug!("[main] torrent: {:?}", torrent);
}

#[derive(Debug)]
struct BitTorrent {
    decoder: Decoder,
    metadata: Option<HashMap<BencodePrimitiveTypes, BencodePrimitiveTypes>>
}

impl BitTorrent {
    pub fn new(decoder: Decoder) -> Self {
        Self { decoder, metadata: None }
    }

    pub fn load_file(&mut self,path: &str) -> Result<()>{
        let bytes = fs::read(path)?;
        debug!("[BitTorrent] bytes length loaded from file: {:?}", bytes.len());

        let (n, val) = self.decoder.from_bytes(&bytes)?;
        debug!("[BitTorrent] bytes length decoded: {:?}", n);

        match val {
            BencodeType::Dictionary(val) => {
                self.metadata = Some(val);
            }
            _ => {
                return Err(anyhow!("the provided data is not a valid torrent file"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq, Eq, Hash)]
enum BencodePrimitiveTypes {
    String(String),
    Integer(isize),
}

#[derive(Debug, PartialEq)]
enum BencodeType {
    Primitive(BencodePrimitiveTypes),
    List(Vec<BencodePrimitiveTypes>),
    Dictionary(HashMap<BencodePrimitiveTypes, BencodePrimitiveTypes>),
}

#[derive(Debug, PartialEq)]
struct Decoder {}

impl Decoder {
    // Returns the number of bytes read and the decoded type
    pub fn from_bytes(&self, bytes: &[u8]) -> Result<(usize, BencodeType)>{
        if is_list(bytes) {
            let (n, val) = self.decode_list(bytes)?;
            return Ok((n, BencodeType::List(val)));
        } 

        if is_dictionary(bytes) {
            let (n, val) = self.decode_dictionary(bytes)?;
            return Ok((n, BencodeType::Dictionary(val)));
        }

        let (n, val) = self.decode_primitive(bytes)?;
        return Ok((n, BencodeType::Primitive(val)));
    }

    pub fn decode_primitive(&self, bytes: &[u8]) -> Result<(usize, BencodePrimitiveTypes)> {
        if is_string(bytes) {
            let (n, val) = self.decode_string(bytes)?;
            return Ok((n, BencodePrimitiveTypes::String(val)));
        }
        
        if is_integer(bytes) {
            let (n, val) = self.decode_integer(bytes)?;
            return Ok((n, BencodePrimitiveTypes::Integer(val)));
        }

        Err(anyhow!("received an unsupported type"))
    }

    pub fn decode_string(&self, bytes: &[u8]) -> Result<(usize, String)> {
        if !is_string(bytes) {
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
        let input = &bytes[curr_idx..len+curr_idx];
        let str = std::str::from_utf8(input)?;

        dbg!("[decode_string] extracted string: {:?}", str);

        let n = len + curr_idx; // considers the len of the string in the message and the colon
        
        Ok((n, str.to_string()))
    }
    
    pub fn decode_integer(&self, bytes: &[u8]) -> Result<(usize, isize)> {
        if !is_integer(bytes) {
            return Err(anyhow!("the provided data is not a integer"));
        }

        let mut end = 0;
        loop {
            if bytes[end] as char == 'e'{
                break;
            }
            end+=1
        }
    
        let input = &bytes[1..end];
        let str = std::str::from_utf8(input).expect("Invalid UTF-8 string");
        let number = str.parse::<isize>()?;
        let n = 1 + end; // 1 because if the 'i'. The end already considers the 'e'.
        Ok((n, number))
    }
    
    pub fn decode_list(&self, bytes: &[u8]) -> Result<(usize, Vec<BencodePrimitiveTypes>)> {
        if !is_list(bytes) {
            return Err(anyhow!("the provided data is not a list"));
        }
    
        let input = &bytes[1..bytes.len()-1];
    
        let mut result = Vec::new();
        let mut curr_idx = 0;

        while curr_idx < input.len() {
            let (n, val) = self.decode_primitive(&input[curr_idx..])?;
            result.push(val);
            curr_idx += n;
        }

        // +2 because of the 'l' and the 'e' in the provided bytes
        Ok((curr_idx + 2, result))
    }

    pub fn decode_dictionary(&self, bytes: &[u8]) -> Result<(usize, HashMap<BencodePrimitiveTypes, BencodePrimitiveTypes>)> {
        if !(is_dictionary(bytes)) {
            return Err(anyhow!("the provided data is not a dictionary"));
        }

        let input = &bytes[1..bytes.len()-1]; // the 'd' and the 'e' in the provided bytes

        let mut curr_idx = 0;
        let mut hm = HashMap::new();
        while curr_idx < input.len(){
            let (n, key) = self.decode_primitive(&input[curr_idx..])?;
            curr_idx+=n;

            let (n, val) = self.decode_primitive(&input[curr_idx..])?;
            curr_idx += n;

            hm.insert(key, val);
        }

        Ok((curr_idx+2, hm)) // +2 because of the 'd' and the 'e' in the provided bytes
    }
}

fn is_list(bytes: &[u8]) -> bool {
    return bytes.len() >= 2 && bytes[0] as char == 'l' && bytes[bytes.len()-1] as char == 'e'
}

fn is_string(bytes: &[u8]) -> bool {
    let mut curr_idx = 0;
    
    while curr_idx < bytes.len() {
        if bytes[curr_idx] as char == ':' {
            break;
        }
        curr_idx += 1;
    }

    return bytes.len() >= 2 && bytes[0] as isize > 0 && curr_idx < bytes.len()
}

fn is_integer(bytes: &[u8]) -> bool {
    return bytes.len() >= 2 && bytes[0] as char =='i' && bytes[bytes.len()-1] as char == 'e'
}

fn is_dictionary(bytes: &[u8]) -> bool {
    return bytes.len() >= 2 && bytes[0] as char == 'd' && bytes[bytes.len()-1] as char == 'e'
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
            )
        ];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let decoder = Decoder {};
            let res= decoder.decode_string(&input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(res.unwrap_err().to_string().contains(&expected.unwrap_err().to_string()));
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
                assert!(res.unwrap_err().to_string().contains(&expected.unwrap_err().to_string()));
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
                Ok((25, vec![BencodePrimitiveTypes::String(String::from("rust")), BencodePrimitiveTypes::String(String::from("golang")), BencodePrimitiveTypes::Integer(40), BencodePrimitiveTypes::Integer(-60)])),
            ),
            (
                "02 - empty list",
                "le".as_bytes().to_vec(),
                Ok((2, vec![])),
            ),
            (
                "03 - list with a single string",
                "l5:helloi52ee".as_bytes().to_vec(),
                Ok((13, vec![BencodePrimitiveTypes::String(String::from("hello")), BencodePrimitiveTypes::Integer(52)])),
            ),
            (
                "04 - list with a single integer",
                "li40ee".as_bytes().to_vec(),
                Ok((6, vec![BencodePrimitiveTypes::Integer(40)])),
            ),
        ];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let decoder = Decoder {};
            let res = decoder.decode_list(&input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(res.unwrap_err().to_string().contains(&expected.unwrap_err().to_string()));
            } else {
                panic!("Result variants did not match");
            }
        }
    }

    #[test]
    fn test_decode_dictionary() {
        let test_cases = vec![
            (
                "01 - simple dictionary",
                "d3:foo3:bar5:helloi52ee".as_bytes().to_vec(),
                Ok((23, HashMap::from([(BencodePrimitiveTypes::String(String::from("foo")), BencodePrimitiveTypes::String(String::from("bar"))), (BencodePrimitiveTypes::String(String::from("hello")), BencodePrimitiveTypes::Integer(52))]))),
            ),
        ];

        for (name, input, expected) in test_cases {
            println!("{}", name);

            let decoder = Decoder {};
            let res = decoder.decode_dictionary(&input);

            if res.is_ok() && expected.is_ok() {
                assert_eq!(res.unwrap(), expected.unwrap());
            } else if res.is_err() && expected.is_err() {
                assert!(res.unwrap_err().to_string().contains(&expected.unwrap_err().to_string()));
            } else {
                panic!("Result variants did not match");
            }
        }
    }
}