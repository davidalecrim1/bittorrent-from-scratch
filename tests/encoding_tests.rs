use bittorrent_from_scratch::encoding::{Decoder, Encoder};
use bittorrent_from_scratch::types::BencodeTypes;
use std::collections::BTreeMap;

#[test]
fn test_encode_decode_roundtrip_string() {
    let encoder = Encoder {};
    let decoder = Decoder {};

    let original = BencodeTypes::String("hello world".to_string());
    let encoded = encoder.from_bencode_types(original.clone()).unwrap();
    let (_, decoded) = decoder.from_bytes(&encoded).unwrap();

    assert_eq!(original, decoded);
}

#[test]
fn test_encode_decode_roundtrip_integer() {
    let encoder = Encoder {};
    let decoder = Decoder {};

    let original = BencodeTypes::Integer(42);
    let encoded = encoder.from_bencode_types(original.clone()).unwrap();
    let (_, decoded) = decoder.from_bytes(&encoded).unwrap();

    assert_eq!(original, decoded);
}

#[test]
fn test_encode_decode_roundtrip_negative_integer() {
    let encoder = Encoder {};
    let decoder = Decoder {};

    let original = BencodeTypes::Integer(-123);
    let encoded = encoder.from_bencode_types(original.clone()).unwrap();
    let (_, decoded) = decoder.from_bytes(&encoded).unwrap();

    assert_eq!(original, decoded);
}

#[test]
fn test_encode_decode_roundtrip_list() {
    let encoder = Encoder {};
    let decoder = Decoder {};

    let original = BencodeTypes::List(vec![
        BencodeTypes::String("foo".to_string()),
        BencodeTypes::Integer(123),
        BencodeTypes::String("bar".to_string()),
    ]);
    let encoded = encoder.from_bencode_types(original.clone()).unwrap();
    let (_, decoded) = decoder.from_bytes(&encoded).unwrap();

    assert_eq!(original, decoded);
}

#[test]
fn test_encode_decode_roundtrip_dictionary() {
    let encoder = Encoder {};
    let decoder = Decoder {};

    let mut dict = BTreeMap::new();
    dict.insert("name".to_string(), BencodeTypes::String("test".to_string()));
    dict.insert("size".to_string(), BencodeTypes::Integer(1024));

    let original = BencodeTypes::Dictionary(dict);
    let encoded = encoder.from_bencode_types(original.clone()).unwrap();
    let (_, decoded) = decoder.from_bytes(&encoded).unwrap();

    assert_eq!(original, decoded);
}

#[test]
fn test_encode_decode_nested_structure() {
    let encoder = Encoder {};
    let decoder = Decoder {};

    let mut inner_dict = BTreeMap::new();
    inner_dict.insert("key1".to_string(), BencodeTypes::Integer(100));

    let mut outer_dict = BTreeMap::new();
    outer_dict.insert("nested".to_string(), BencodeTypes::Dictionary(inner_dict));
    outer_dict.insert(
        "list".to_string(),
        BencodeTypes::List(vec![BencodeTypes::String("item".to_string())]),
    );

    let original = BencodeTypes::Dictionary(outer_dict);
    let encoded = encoder.from_bencode_types(original.clone()).unwrap();
    let (_, decoded) = decoder.from_bytes(&encoded).unwrap();

    assert_eq!(original, decoded);
}

#[test]
fn test_decode_empty_string() {
    let decoder = Decoder {};
    let input = b"0:";
    let (_, decoded) = decoder.from_bytes(input).unwrap();

    assert_eq!(decoded, BencodeTypes::String("".to_string()));
}

#[test]
fn test_decode_zero() {
    let decoder = Decoder {};
    let input = b"i0e";
    let (_, decoded) = decoder.from_bytes(input).unwrap();

    assert_eq!(decoded, BencodeTypes::Integer(0));
}

#[test]
fn test_decode_empty_list() {
    let decoder = Decoder {};
    let input = b"le";
    let (_, decoded) = decoder.from_bytes(input).unwrap();

    assert_eq!(decoded, BencodeTypes::List(vec![]));
}

#[test]
fn test_decode_empty_dictionary() {
    let decoder = Decoder {};
    let input = b"de";
    let (_, decoded) = decoder.from_bytes(input).unwrap();

    assert_eq!(decoded, BencodeTypes::Dictionary(BTreeMap::new()));
}
