// Copyright 2019 Mozilla Foundation. See the COPYRIGHT
// file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use chardet::UniversalDetector;
use chardetng::EncodingDetector;
use detone::IterDecomposeVietnamese;
use encoding_rs::Encoding;
use encoding_rs::IBM866;
use encoding_rs::ISO_8859_2;
use encoding_rs::ISO_8859_5;
use encoding_rs::KOI8_U;
use encoding_rs::WINDOWS_1250;
use encoding_rs::WINDOWS_1251;
use encoding_rs::WINDOWS_1252;
use encoding_rs::WINDOWS_1254;
use encoding_rs::WINDOWS_1258;

#[link(name = "stdc++", kind = "static")]
extern "C" {}

#[link(name = "ced", kind = "static")]
extern "C" {
    fn compact_enc_det_detect(text: *const u8, text_len: usize, name_len: *mut usize) -> *const u8;
}

fn ced(buffer: &[u8]) -> &'static Encoding {
    unsafe {
        let mut name_len = 0usize;
        let name_ptr = compact_enc_det_detect(buffer.as_ptr(), buffer.len(), &mut name_len);
        let name = std::slice::from_raw_parts(name_ptr, name_len);
        Encoding::for_label(name).unwrap_or(WINDOWS_1252)
    }
}

fn check_ced(encoding: &'static Encoding, bytes: &[u8]) -> bool {
    let detected = ced(&bytes);
    let (expected, _) = encoding.decode_without_bom_handling(&bytes);
    let (actual, _) = detected.decode_without_bom_handling(&bytes);
    // println!("{:?}", detected);
    expected == actual
}

fn chardet(buffer: &[u8]) -> &'static Encoding {
    let mut chardet = UniversalDetector::new();
    chardet.feed(buffer);
    let (name, _, _) = chardet.close();
    Encoding::for_label(name.as_bytes()).unwrap_or(WINDOWS_1252)
}

fn check_chardet(encoding: &'static Encoding, bytes: &[u8]) -> bool {
    let detected = ced(&bytes);
    let (expected, _) = encoding.decode_without_bom_handling(&bytes);
    let (actual, _) = detected.decode_without_bom_handling(&bytes);
    // println!("{:?}", detected);
    expected == actual
}

fn ng(buffer: &[u8], det: &mut EncodingDetector) -> &'static Encoding {
    let (enc, _, _) = det.feed(buffer, true);
    enc
}

fn check_ng(
    encoding: &'static Encoding,
    bytes: &[u8],
) -> Option<(&'static Encoding, String, i64, String, i64, bool)> {
    let mut det = EncodingDetector::new();
    let detected = ng(&bytes, &mut det);
    let (expected, _) = encoding.decode_without_bom_handling(&bytes);
    let (actual, _) = detected.decode_without_bom_handling(&bytes);
    // println!("{:?}", detected);
    if expected == actual {
        return None;
    }
    let (detected_score, _) = det.find_score(detected);
    let (expected_score, expected_disqualified) = det.find_score(encoding);
    Some((
        detected,
        actual.into_owned(),
        detected_score,
        expected.into_owned(),
        expected_score,
        expected_disqualified,
    ))
}

fn encode<'a>(s: &'a str, encoding: &'static Encoding) -> Option<Vec<u8>> {
    if Encoding::ascii_valid_up_to(s.as_bytes()) == s.len() {
        return None;
    }
    let bytes = if encoding == WINDOWS_1258 {
        let preprocessed = s
            .chars()
            .decompose_vietnamese_tones(true)
            .collect::<String>();
        let (bytes, _, _) = encoding.encode(&preprocessed);
        if Encoding::ascii_valid_up_to(&bytes) == bytes.len() {
            return None;
        }
        Some(bytes.into_owned())
    } else if encoding == WINDOWS_1250 || encoding == ISO_8859_2 {
        let preprocessed = s
            .chars()
            .map(|c| match c {
                'ț' => 'ţ',
                'ș' => 'ş',
                'Ț' => 'Ţ',
                'Ș' => 'Ş',
                _ => c,
            })
            .collect::<String>();
        let (bytes, _, _) = encoding.encode(&preprocessed);
        if Encoding::ascii_valid_up_to(&bytes) == bytes.len() {
            return None;
        }
        Some(bytes.into_owned())
    } else if encoding == WINDOWS_1254 {
        let preprocessed = s
            .chars()
            .map(|c| match c {
                'Ə' => 'Ä',
                'ə' => 'ä',
                _ => c,
            })
            .collect::<String>();
        let (bytes, _, _) = encoding.encode(&preprocessed);
        if Encoding::ascii_valid_up_to(&bytes) == bytes.len() {
            return None;
        }
        Some(bytes.into_owned())
    } else if encoding == WINDOWS_1251
        || encoding == ISO_8859_5
        || encoding == IBM866
        || encoding == KOI8_U
    {
        let preprocessed = s
            .chars()
            .map(|c| match c {
                'Ү' => 'Ї',
                'ү' => 'ї',
                'Ө' => 'Є',
                'ө' => 'є',
                _ => c,
            })
            .collect::<String>();
        let (bytes, _, _) = encoding.encode(&preprocessed);
        if Encoding::ascii_valid_up_to(&bytes) == bytes.len() {
            return None;
        }
        Some(bytes.into_owned())
    } else {
        let (bytes, _, _) = encoding.encode(s);
        if Encoding::ascii_valid_up_to(&bytes) == bytes.len() {
            return None;
        }
        Some(bytes.into_owned())
    };
    bytes
}

fn check(s: &str, encoding: &'static Encoding) {
    if let Some(bytes) = encode(s, encoding) {
        if let Some((
            detected,
            actual_text,
            detected_score,
            expected_text,
            expected_score,
            expected_disqualified,
        )) = check_ng(encoding, &bytes)
        {
            let chardet = check_chardet(encoding, &bytes);
            let ced = check_ced(encoding, &bytes);
            if !chardet && !ced {
                // Competition failed, too.
                return;
            }
            println!("Expected: {} (score: {}, disqualified: {}), got: {} (score {}), ced {}, chardet {}, input: {}, output: {}", encoding.name(), expected_score, expected_disqualified, detected.name(), detected_score, if ced { "ok" } else { "FAIL" }, if chardet { "ok" } else { "FAIL" }, expected_text, actual_text);
        }
    }
}

fn main() {
    check("Русский ", ISO_8859_5);
}
