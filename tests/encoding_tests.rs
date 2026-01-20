use bittorrent_from_scratch::encoding::{BencodeTypes, Decoder, Encoder};
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

#[test]
fn test_encode_raw_bytes() {
    let encoder = Encoder {};

    let raw_data = vec![0x01, 0x02, 0x03, 0x04];
    let original = BencodeTypes::Raw(raw_data.clone());
    let encoded = encoder.from_bencode_types(original).unwrap();

    let expected = b"4:\x01\x02\x03\x04".to_vec();
    assert_eq!(encoded, expected);
}

#[test]
fn test_encode_pieces_field_with_list_of_hashes() {
    let encoder = Encoder {};

    // SHA1 hashes are 20 bytes each, represented as hex strings
    let hash1 = "0123456789abcdef0123456789abcdef01234567";
    let hash2 = "fedcba9876543210fedcba9876543210fedcba98";

    let mut dict = BTreeMap::new();
    dict.insert(
        "pieces".to_string(),
        BencodeTypes::List(vec![
            BencodeTypes::String(hash1.to_string()),
            BencodeTypes::String(hash2.to_string()),
        ]),
    );

    let original = BencodeTypes::Dictionary(dict);
    let encoded = encoder.from_bencode_types(original).unwrap();

    // Should encode as "d6:pieces40:<binary_data>e"
    // Verify it starts with dictionary and pieces key
    assert!(encoded.starts_with(b"d6:pieces40:"));
    assert!(encoded.ends_with(b"e"));
}

#[test]
fn test_encode_pieces_field_with_raw_bytes() {
    let encoder = Encoder {};

    // Raw binary hash data (20 bytes for one SHA1 hash)
    let raw_hash = vec![
        0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0xab, 0xcd,
        0xef, 0x01, 0x23, 0x45, 0x67,
    ];

    let mut dict = BTreeMap::new();
    dict.insert("pieces".to_string(), BencodeTypes::Raw(raw_hash.clone()));

    let original = BencodeTypes::Dictionary(dict);
    let encoded = encoder.from_bencode_types(original).unwrap();

    // Should encode as "d6:pieces20:<binary_data>e"
    assert!(encoded.starts_with(b"d6:pieces20:"));
    assert!(encoded.ends_with(b"e"));
}

#[test]
fn test_encode_pieces_field_error_on_invalid_type() {
    let encoder = Encoder {};

    // pieces field should be list or raw, not integer
    let mut dict = BTreeMap::new();
    dict.insert("pieces".to_string(), BencodeTypes::Integer(123));

    let original = BencodeTypes::Dictionary(dict);
    let result = encoder.from_bencode_types(original);

    assert!(result.is_err());
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("not a list or a string")
    );
}

#[test]
fn test_encode_pieces_list_error_on_non_string_item() {
    let encoder = Encoder {};

    // pieces list should contain strings (hex hashes), not integers
    let mut dict = BTreeMap::new();
    dict.insert(
        "pieces".to_string(),
        BencodeTypes::List(vec![BencodeTypes::Integer(123)]),
    );

    let original = BencodeTypes::Dictionary(dict);
    let result = encoder.from_bencode_types(original);

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not a string"));
}
