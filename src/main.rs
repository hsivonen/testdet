// Copyright Mozilla Foundation. See the COPYRIGHT
// file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use bzip2::bufread::BzDecoder;
use encoding_rs::DecoderResult;
use encoding_rs::BIG5;
use encoding_rs::BIG5_INIT;
use encoding_rs::EUC_JP_INIT;
use encoding_rs::EUC_KR;
use encoding_rs::EUC_KR_INIT;
use encoding_rs::GBK;
use encoding_rs::GBK_INIT;
use encoding_rs::ISO_8859_13_INIT;
use encoding_rs::SHIFT_JIS_INIT;
use quick_xml::events::Event;
use regex::Regex;
use std::borrow::Cow;

use encoding_rs::ISO_8859_8;

use encoding_rs::X_USER_DEFINED;
use rayon::prelude::*;
use std::io::BufRead;
use std::path::Path;
use std::path::PathBuf;
use unicode_reverse::reverse_grapheme_clusters_in_place;

use chardet::UniversalDetector;
use chardetng::EncodingDetector;
use detone::IterDecomposeVietnamese;
use encoding_rs::Encoding;
use encoding_rs::IBM866;
use encoding_rs::IBM866_INIT;
use encoding_rs::ISO_8859_2;
use encoding_rs::ISO_8859_2_INIT;
use encoding_rs::ISO_8859_4_INIT;
use encoding_rs::ISO_8859_5;
use encoding_rs::ISO_8859_5_INIT;
use encoding_rs::ISO_8859_6_INIT;
use encoding_rs::ISO_8859_7_INIT;
use encoding_rs::ISO_8859_8_INIT;
use encoding_rs::KOI8_U;
use encoding_rs::KOI8_U_INIT;
use encoding_rs::WINDOWS_1250_INIT;
use encoding_rs::WINDOWS_1251_INIT;
use encoding_rs::WINDOWS_1252_INIT;
use encoding_rs::WINDOWS_1253_INIT;
use encoding_rs::WINDOWS_1254_INIT;
use encoding_rs::WINDOWS_1255_INIT;
use encoding_rs::WINDOWS_1256_INIT;
use encoding_rs::WINDOWS_1257_INIT;
use encoding_rs::WINDOWS_1258_INIT;
use encoding_rs::WINDOWS_874_INIT;

use encoding_rs::WINDOWS_1250;
use encoding_rs::WINDOWS_1251;
use encoding_rs::WINDOWS_1252;

use encoding_rs::WINDOWS_1254;

use encoding_rs::WINDOWS_1258;
use libflate::gzip::Decoder;
use std::fs::File;
use std::io::BufReader;
use std::process::Command;
use unic_normal::StrNormalForm;

static ENCODINGS: [&'static Encoding; 19] = [
    &WINDOWS_1250_INIT,
    &WINDOWS_1251_INIT,
    &WINDOWS_1252_INIT,
    &WINDOWS_1253_INIT,
    &WINDOWS_1254_INIT,
    &WINDOWS_1255_INIT,
    &WINDOWS_1256_INIT,
    &WINDOWS_1257_INIT,
    &WINDOWS_1258_INIT,
    &WINDOWS_874_INIT,
    &IBM866_INIT,
    &KOI8_U_INIT,
    &ISO_8859_2_INIT,
    &ISO_8859_4_INIT,
    &ISO_8859_5_INIT,
    &ISO_8859_6_INIT,
    &ISO_8859_7_INIT,
    &ISO_8859_8_INIT,
    &ISO_8859_13_INIT,
];

struct FastEncoder {
    tables: [[u8; 0x10000]; 19],
}

impl FastEncoder {
    fn new() -> Self {
        let mut instance = FastEncoder {
            tables: [[0; 0x10000]; 19],
        };
        for (j, encoding) in ENCODINGS.iter().enumerate() {
            let mut decoder = encoding.new_decoder_without_bom_handling();
            for i in 128..256 {
                let mut output = [0u16; 2];
                let input = [i as u8; 1];
                let (result, read, written) =
                    decoder.decode_to_utf16_without_replacement(&input, &mut output, false);
                match result {
                    DecoderResult::OutputFull => {
                        unreachable!("Should never be full.");
                    }
                    DecoderResult::Malformed(_, _) => {}
                    DecoderResult::InputEmpty => {
                        assert_eq!(read, 1);
                        assert_eq!(written, 1);
                        instance.tables[j][output[0] as usize] = i as u8;
                    }
                }
            }
        }
        instance
    }

    fn encode<'a>(&self, encoding: &'static Encoding, s: &'a str) -> Cow<'a, [u8]> {
        let i = ENCODINGS.iter().position(|&x| x == encoding).unwrap();
        if Encoding::ascii_valid_up_to(s.as_bytes()) == s.len() {
            return Cow::Borrowed(s.as_bytes());
        }
        let table: &[u8; 0x10000] = &self.tables[i];
        let mut vec = Vec::with_capacity(s.len());
        for c in s.chars() {
            if c < '\u{80}' {
                vec.push(c as u8);
            } else if c < '\u{10000}' {
                let b = table[c as usize];
                if b == 0 {
                    vec.extend_from_slice(b"&#;");
                } else {
                    vec.push(b);
                }
            } else {
                vec.extend_from_slice(b"&#;");
            }
        }
        Cow::Owned(vec)
    }
}

fn find_file(dir: &Path, lang: &str, full_articles: bool) -> PathBuf {
    for entry in dir.read_dir().expect("Reading the title directory failed.") {
        if let Ok(entry) = entry {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if s.starts_with(lang) && s.ends_with(if full_articles { ".bz2" } else { ".gz" }) {
                return entry.path();
            }
        }
    }
    if full_articles {
        eprintln!("Error: No articles for: {}", lang);
    } else {
        eprintln!("Error: No titles for: {}", lang);
    }
    std::process::exit(-4);
}

fn test_lang(
    path: &Path,
    tld: Option<&[u8]>,
    enc: &'static Encoding,
    orthographic: bool,
    print: bool,
    score_card: &mut ScoreCard,
    fast_encoder: &FastEncoder,
    mode: CheckMode,
    max_non_ascii: usize,
    chunk: usize,
) {
    let media_wiki_special =
        Regex::new(r"^(?:\u{200D}\u{200C})?\p{Alphabetic}+:\p{Alphabetic}+$").unwrap();
    let mut read = BufReader::new(Decoder::new(BufReader::new(File::open(path).unwrap())).unwrap());
    let mut buf = String::new();
    loop {
        buf.clear();
        let num_read = read.read_line(&mut buf).unwrap();
        if num_read == 0 {
            return;
        }
        let end = if buf.as_bytes()[buf.len() - 1] == b'\n' {
            buf.len() - 1
        } else {
            buf.len()
        };
        let s = &buf[..end];
        if media_wiki_special.is_match(s) {
            continue;
        }
        check(
            s,
            tld,
            enc,
            orthographic,
            print,
            score_card,
            &fast_encoder,
            mode,
            max_non_ascii,
            chunk,
        );
    }
}

fn test_lang_full(
    path: &Path,
    tld: Option<&[u8]>,
    enc: &'static Encoding,
    orthographic: bool,
    print: bool,
    score_card: &mut ScoreCard,
    fast_encoder: &FastEncoder,
    mode: CheckMode,
    max_non_ascii: usize,
    chunk: usize,
) {
    let mut xml = quick_xml::Reader::from_reader(BufReader::new(BzDecoder::new(BufReader::new(
        File::open(path).unwrap(),
    ))));
    let mut text = String::new();
    let mut buf = Vec::new();
    text.clear();
    let mut text_open = false;
    loop {
        match xml.read_event(&mut buf) {
            Ok(Event::Start(ref e)) => match e.name() {
                b"text" => {
                    assert!(!text_open);
                    text_open = true;
                    text.clear();
                }
                _ => {}
            },
            Ok(Event::End(ref e)) => match e.name() {
                b"text" => {
                    assert!(text_open);
                    if text.len() > 6000 {
                        check(
                            &text,
                            tld,
                            enc,
                            orthographic,
                            print,
                            score_card,
                            &fast_encoder,
                            mode,
                            max_non_ascii,
                            chunk,
                        );
                    }
                    text.clear();
                    text_open = false;
                }
                _ => {}
            },
            Ok(Event::Text(e)) | Ok(Event::CData(e)) => {
                if text_open {
                    text.push_str(&e.unescape_and_decode(&xml).unwrap());
                }
            }
            Err(e) => panic!("XML error {}: {:?}", xml.buffer_position(), e),
            Ok(Event::Eof) => break,
            _ => {}
        }
        buf.clear();
    }
}

#[derive(Debug)]
struct EncodingClass {
    encodings: &'static [&'static Encoding],
    languages: &'static [(&'static str, &'static str)],
    name: &'static str,
}

static ENCODING_CLASSES: [EncodingClass; 18] = [
    // Vietnamese consumes the corpus twice, so put it first
    // to maximize parallelism.
    // In the `encodings` field, the Windows encoding comes first.
    EncodingClass {
        encodings: &[&WINDOWS_1258_INIT],
        languages: &[("vi", "vi")],
        name: "vietnamese",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1250_INIT, &ISO_8859_2_INIT],
        languages: &[
            ("pl", "pl"),
            ("hu", "hu"),
            ("sh", "hr"),
            ("cs", "cz"),
            ("ro", "ro"),
            ("sk", "sk"),
            ("hr", "hr"),
            ("sl", "si"),
            ("bs", "ba"),
        ],
        name: "central",
    },
    EncodingClass {
        // IE and Chromium don't detect x-mac-cyrillic.
        encodings: &[
            &WINDOWS_1251_INIT,
            &KOI8_U_INIT,
            &ISO_8859_5_INIT,
            &IBM866_INIT,
        ],
        // kk, tt, tg, and os don't fit
        // mn uses mapping to uk letters
        languages: &[("ru", "ru"), ("ce", "ru")],
        name: "russia",
    },
    EncodingClass {
        // IE and Chromium don't detect x-mac-cyrillic.
        encodings: &[&WINDOWS_1251_INIT, &KOI8_U_INIT, &ISO_8859_5_INIT],
        // kk, tt, tg, and os don't fit
        // mn uses mapping to uk letters
        languages: &[("sr", "rs"), ("bg", "bg"), ("be", "by"), ("mk", "mk")],
        name: "cyrillic-iso",
    },
    EncodingClass {
        // IE and Chromium don't detect x-mac-cyrillic.
        encodings: &[&WINDOWS_1251_INIT, &KOI8_U_INIT],
        // kk, tt, tg, and os don't fit
        // mn uses mapping to uk letters
        languages: &[("uk", "ua"), ("mn", "mn")],
        name: "ukrainian",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1252_INIT],
        // Intentionally omitting ASCII languages like en, nl, id, so, sw, various Malay-alphabet languages
        languages: &[
            ("sv", "se"),
            ("de", "de"),
            ("fr", "fr"),
            ("it", "it"),
            ("es", "es"),
            ("pt", "pt"),
            ("ca", "es"),
            ("no", "no"),
            ("fi", "fi"),
            ("eu", "es"),
            ("da", "dk"),
            ("gl", "es"),
            ("nn", "no"),
            ("oc", "fr"),
            ("br", "fr"),
            ("lb", "lu"),
            ("ht", "ht"),
            ("ga", "es"),
            ("is", "is"),
            ("an", "es"),
            ("wa", "be"),
            ("gd", "uk"),
            ("fo", "fo"),
            ("li", "be"),
            ("sq", "al"),
        ],
        name: "western",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1253_INIT, &ISO_8859_7_INIT],
        languages: &[("el", "gr")],
        name: "greek",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1254_INIT],
        languages: &[("tr", "tr"), ("az", "az"), ("ku", "tr")],
        name: "turkish",
    },
    EncodingClass {
        encodings: &[
            &WINDOWS_1255_INIT, // , &ISO_8859_8_INIT
        ],
        languages: &[("he", "il"), ("yi", "il")],
        name: "hebrew",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1256_INIT, &ISO_8859_6_INIT],
        languages: &[("ar", "sa")],
        name: "arabic",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1256_INIT],
        languages: &[("fa", "ir"), ("ur", "pk")],
        name: "persian-urdu",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1252_INIT, &WINDOWS_1257_INIT, &ISO_8859_4_INIT],
        languages: &[("et", "ee")],
        name: "estonian",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1257_INIT, &ISO_8859_4_INIT],
        languages: &[("lt", "lt"), ("lv", "lv")],
        name: "baltic",
    },
    EncodingClass {
        encodings: &[&WINDOWS_874_INIT],
        languages: &[("th", "th")],
        name: "thai",
    },
    EncodingClass {
        encodings: &[&SHIFT_JIS_INIT, &EUC_JP_INIT],
        languages: &[("ja", "jp")],
        name: "japanese",
    },
    EncodingClass {
        encodings: &[&EUC_KR_INIT],
        languages: &[("ko", "kr")],
        name: "korean",
    },
    EncodingClass {
        encodings: &[&GBK_INIT],
        languages: &[("zh-hans", "cn")],
        name: "simplified",
    },
    EncodingClass {
        encodings: &[&BIG5_INIT],
        languages: &[("zh-hant", "tw")],
        name: "traditional",
    },
];

fn test_one(
    lang: &str,
    tld: Option<&[u8]>,
    dir: &Path,
    enc: &'static Encoding,
    orthographic: bool,
    print: bool,
    fast_encoder: &FastEncoder,
    full_articles: bool,
    mode: CheckMode,
    max_non_ascii: usize,
    chunk: usize,
) -> ScoreCard {
    let path = find_file(dir, lang, full_articles);
    let mut score_card = ScoreCard::new();
    if full_articles {
        test_lang_full(
            &path,
            tld,
            enc,
            orthographic,
            print,
            &mut score_card,
            fast_encoder,
            mode,
            max_non_ascii,
            chunk,
        );
    } else {
        test_lang(
            &path,
            tld,
            enc,
            orthographic,
            print,
            &mut score_card,
            fast_encoder,
            mode,
            max_non_ascii,
            chunk,
        );
    }
    score_card
}

struct ScoreCard {
    total: u64,
    ng: u64,
    ced: u64,
    chardet: u64,
    icu: u64,
}

impl ScoreCard {
    fn new() -> Self {
        ScoreCard {
            total: 0,
            ng: 0,
            ced: 0,
            chardet: 0,
            icu: 0,
        }
    }

    fn print(&self, lang: &str, encoding: &'static Encoding, orthographic: bool) {
        let mut winner = "ng";
        if self.ced > self.ng {
            winner = "ced";
        }
        if self.chardet > self.ced && self.chardet > self.ng {
            winner = "chardet";
        }
        if self.icu > self.chardet && self.icu > self.ced && self.icu > self.ng {
            winner = "icu";
        }
        let total_float = self.total as f64;
        let ng_prop = (self.ng as f64) / total_float;
        let ced_prop = (self.ced as f64) / total_float;
        let chardet_prop = (self.chardet as f64) / total_float;
        let icu_prop = (self.icu as f64) / total_float;
        let orth = if !orthographic && encoding == WINDOWS_1258 {
            " (non-orthographic)"
        } else {
            ""
        };
        println!(
            "{}\t{}{}\twin:\t{}\tng:\t{:.2}\tced:\t{:.2}\tchardet:\t{:.2}\ticu:\t{:.2}",
            lang,
            encoding.name(),
            orth,
            winner,
            ng_prop,
            ced_prop,
            chardet_prop,
            icu_prop
        );
    }

    fn add(&mut self, other: &ScoreCard) {
        self.total += other.total;
        self.ng += other.ng;
        self.ced += other.ced;
        self.chardet += other.chardet;
        self.icu += other.icu;
    }
}

#[link(name = "stdc++", kind = "static")]
extern "C" {}

#[link(name = "ced", kind = "static")]
extern "C" {
    fn compact_enc_det_detect(text: *const u8, text_len: usize, name_len: *mut usize) -> *const u8;
}

#[link(name = "icui18n")]
extern "C" {
    fn ucsdet_open_60(error: *mut libc::c_int) -> *mut libc::c_void;
    fn ucsdet_setText_60(
        det: *mut libc::c_void,
        buf: *const u8,
        buf_len: i32,
        error: *mut libc::c_int,
    );
    fn ucsdet_enableInputFilter_60(det: *mut libc::c_void, enabled: bool) -> bool;
    fn ucsdet_detect_60(det: *mut libc::c_void, error: *mut libc::c_int) -> *mut libc::c_void;
    fn ucsdet_getName_60(guess: *mut libc::c_void, error: *mut libc::c_int) -> *const libc::c_char;
    fn ucsdet_close_60(det: *mut libc::c_void);
}

fn icu(buffer: &[u8]) -> &'static Encoding {
    unsafe {
        let mut err = 0;
        let det = ucsdet_open_60(&mut err);
        ucsdet_enableInputFilter_60(det, true);
        ucsdet_setText_60(det, buffer.as_ptr(), buffer.len() as i32, &mut err);
        let guess = ucsdet_detect_60(det, &mut err);
        let ret = if guess.is_null() {
            WINDOWS_1252
        } else {
            let name_ptr = ucsdet_getName_60(guess, &mut err);
            let name_len = libc::strlen(name_ptr);
            let name = std::slice::from_raw_parts(name_ptr as *const u8, name_len);
            Encoding::for_label(name).unwrap_or(WINDOWS_1252)
        };
        ucsdet_close_60(det);
        ret
    }
}

fn check_icu(encoding: &'static Encoding, bytes: &[u8]) -> bool {
    let detected = icu(&bytes);
    let (expected, _) = encoding.decode_without_bom_handling(&bytes);
    let (actual, _) = detected.decode_without_bom_handling(&bytes);
    // println!("ICU: {:?}", detected);
    expected == actual
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
    let detected = chardet(&bytes);
    let (expected, _) = encoding.decode_without_bom_handling(&bytes);
    let (actual, _) = detected.decode_without_bom_handling(&bytes);
    // println!("{:?}", detected);
    expected == actual
}

fn truncate_by_num_ascii(buffer: &[u8], max_non_ascii: usize) -> &[u8] {
    let mut non_ascii = 0usize;
    for (i, &b) in buffer.iter().enumerate() {
        if non_ascii == max_non_ascii {
            return &buffer[..i];
        }
        if b >= 0x80 {
            non_ascii += 1;
        }
    }
    buffer
}

fn ng(
    buffer: &[u8],
    det: &mut EncodingDetector,
    tld: Option<&[u8]>,
    max_non_ascii: usize,
    chunk: usize,
) -> &'static Encoding {
    let buf = if max_non_ascii == 0 {
        buffer
    } else {
        truncate_by_num_ascii(buffer, max_non_ascii)
    };
    if chunk == 0 || chunk >= buf.len() {
        det.feed(buf, true);
    } else {
        let mut first = chunk > 1024;
        for c in buf.chunks(chunk) {
            if first {
                first = false;
                det.feed(&c[..1024], false);
                det.feed(&c[1024..], false);
            } else {
                det.feed(c, false);
            }
        }
        det.feed(b"", true);
    }
    det.guess(tld, false)
}

fn check_ng(
    tld: Option<&[u8]>,
    encoding: &'static Encoding,
    bytes: &[u8],
    max_non_ascii: usize,
    chunk: usize,
) -> Option<(&'static Encoding, String, i64, String, i64, bool)> {
    let mut det = EncodingDetector::new();
    let detected = ng(&bytes, &mut det, tld, max_non_ascii, chunk);
    let (expected, _) = encoding.decode_without_bom_handling(&bytes);
    let (actual, _) = detected.decode_without_bom_handling(&bytes);
    // println!("{:?}", detected);
    if expected == actual {
        return None;
    }
    let detected_score = det.find_score(detected);
    let expected_score = det.find_score(encoding);
    Some((
        detected,
        actual.into_owned(),
        detected_score.unwrap_or(0),
        expected.into_owned(),
        expected_score.unwrap_or(0),
        expected_score.is_none(),
    ))
}

fn encode<'a>(
    s: &'a str,
    encoding: &'static Encoding,
    orthographic: bool,
    fast_encoder: &FastEncoder,
) -> Option<Vec<u8>> {
    if Encoding::ascii_valid_up_to(s.as_bytes()) == s.len() {
        return None;
    }
    let bytes = if encoding == WINDOWS_1258 {
        let preprocessed = s
            .chars()
            .nfc()
            .decompose_vietnamese_tones(orthographic)
            .map(|c| match c {
                '_' => ' ',
                _ => c,
            })
            .collect::<String>();
        let bytes = if encoding.is_single_byte() {
            fast_encoder.encode(encoding, &preprocessed)
        } else {
            let (bytes, _, _) = encoding.encode(&preprocessed);
            bytes
        };
        if Encoding::ascii_valid_up_to(&bytes) == bytes.len() {
            return None;
        };
        Some(bytes.into_owned())
    } else if encoding == WINDOWS_1250 || encoding == ISO_8859_2 {
        let preprocessed = s
            .chars()
            .nfc()
            .map(|c| match c {
                '_' => ' ',
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
            .nfc()
            .map(|c| match c {
                '_' => ' ',
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
            .nfc()
            .map(|c| match c {
                '_' => ' ',
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
    } else if encoding == BIG5 || encoding == EUC_KR || encoding == GBK {
        for c in s.chars() {
            if c >= '\u{3040}' && c < '\u{3100}' {
                // Reject kana
                return None;
            }
        }
        let preprocessed = s
            .chars()
            .nfc()
            .map(|c| match c {
                '_' => ' ',
                _ => c,
            })
            .collect::<String>();
        let (bytes, _, _) = encoding.encode(&preprocessed);
        if Encoding::ascii_valid_up_to(&bytes) == bytes.len() {
            return None;
        }
        Some(bytes.into_owned())
    } else {
        let preprocessed = s
            .chars()
            .nfc()
            .map(|c| match c {
                '_' => ' ',
                _ => c,
            })
            .collect::<String>();
        let (bytes, _, _) = encoding.encode(&preprocessed);
        if Encoding::ascii_valid_up_to(&bytes) == bytes.len() {
            return None;
        }
        Some(bytes.into_owned())
    };
    bytes
}

#[derive(Eq, PartialEq, Copy, Clone)]
enum CheckMode {
    All,
    Ng,
    Ced,
}

fn check(
    s: &str,
    tld: Option<&[u8]>,
    encoding: &'static Encoding,
    orthographic: bool,
    print: bool,
    score_card: &mut ScoreCard,
    fast_encoder: &FastEncoder,
    mode: CheckMode,
    max_non_ascii: usize,
    chunk: usize,
) {
    let mut string;
    let slice = if encoding == ISO_8859_8 {
        string = s.to_string();
        reverse_grapheme_clusters_in_place(&mut string);
        &string[..]
    } else {
        s
    };

    if let Some(bytes) = encode(slice, encoding, orthographic, fast_encoder) {
        let chardet = if mode == CheckMode::All {
            check_chardet(encoding, &bytes)
        } else {
            true
        };
        let ced = if mode == CheckMode::All || mode == CheckMode::Ced {
            check_ced(encoding, &bytes)
        } else {
            true
        };
        let icu = if mode == CheckMode::All {
            check_icu(encoding, &bytes)
        } else {
            true
        };

        score_card.total += 1;
        score_card.chardet += chardet as u64;
        score_card.ced += ced as u64;
        score_card.icu += icu as u64;

        if mode != CheckMode::Ced {
            if let Some((
                detected,
                actual_text,
                detected_score,
                expected_text,
                expected_score,
                expected_disqualified,
            )) = check_ng(tld, encoding, &bytes, max_non_ascii, chunk)
            {
                if !print {
                    return;
                }
                if !chardet && !ced && !icu {
                    println!("All failed");
                    return;
                }
                println!("Expected: {} (score: {}, disqualified: {}), got: {} (score {}), ced {}, chardet {}, icu {}, input: {}, output: {}", encoding.name(), expected_score, expected_disqualified, detected.name(), detected_score, if ced { "ok" } else { "FAIL" }, if chardet { "ok" } else { "FAIL" }, if icu { "ok" } else { "FAIL" }, expected_text, actual_text);
            } else {
                score_card.ng += 1;
            }
        }
    }
}

fn bench_all(
    dir: &Path,
    print: bool,
    use_tld: bool,
    total_scores: &mut ScoreCard,
    full_articles: bool,
    mode: CheckMode,
    max_non_ascii: usize,
    chunk: usize,
) {
    let fast_encoder = FastEncoder::new();
    // There are likely fancy iterator tricks for this.
    let mut tasks = Vec::new();
    for encoding_class in ENCODING_CLASSES.iter() {
        for (lang, tld) in encoding_class.languages.iter() {
            for &encoding in encoding_class.encodings.iter() {
                tasks.push((lang, tld, encoding, false));
                if encoding == WINDOWS_1258 {
                    tasks.push((lang, tld, encoding, true));
                }
            }
        }
    }
    let score_cards: Vec<ScoreCard> = tasks
        .iter() // Intentionally _not_ Rayon!
        .map(|&task| {
            let (lang, tld, encoding, orthographic) = task;
            let score_card = test_one(
                lang,
                if use_tld { Some(tld.as_bytes()) } else { None },
                dir,
                encoding,
                orthographic,
                print,
                &fast_encoder,
                full_articles,
                mode,
                max_non_ascii,
                chunk,
            );
            score_card.print(lang, encoding, orthographic);
            score_card
        })
        .collect();
    // There are probably fancy tricks for this, too.
    for score_card in score_cards.iter() {
        total_scores.add(score_card);
    }
}

fn test_all(
    dir: &Path,
    print: bool,
    use_tld: bool,
    total_scores: &mut ScoreCard,
    full_articles: bool,
    mode: CheckMode,
    max_non_ascii: usize,
    chunk: usize,
) {
    let fast_encoder = FastEncoder::new();
    // There are likely fancy iterator tricks for this.
    let mut tasks = Vec::new();
    for encoding_class in ENCODING_CLASSES.iter() {
        for (lang, tld) in encoding_class.languages.iter() {
            for &encoding in encoding_class.encodings.iter() {
                tasks.push((lang, tld, encoding, false));
                if encoding == WINDOWS_1258 {
                    tasks.push((lang, tld, encoding, true));
                }
            }
        }
    }
    let score_cards: Vec<ScoreCard> = tasks
        .par_iter()
        .map(|&task| {
            let (lang, tld, encoding, orthographic) = task;
            let score_card = test_one(
                lang,
                if use_tld { Some(tld.as_bytes()) } else { None },
                dir,
                encoding,
                orthographic,
                print,
                &fast_encoder,
                full_articles,
                mode,
                max_non_ascii,
                chunk,
            );
            score_card.print(lang, encoding, orthographic);
            score_card
        })
        .collect();
    // There are probably fancy tricks for this, too.
    for score_card in score_cards.iter() {
        total_scores.add(score_card);
    }
}

fn download_titles(dir: &Path) {
    let prefix = "https://ftp.acc.umu.se/mirror/wikimedia.org/dumps/";
    let date = "20190901";
    let mut curl = Command::new("curl");
    curl.current_dir(dir);
    curl.arg("-L");
    curl.arg("--remote-name-all");
    for encoding_class in ENCODING_CLASSES.iter() {
        for (lang, _) in encoding_class.languages.iter() {
            let mut url = String::new();
            url.push_str(prefix);
            url.push_str(lang);
            url.push_str("wiki/");
            url.push_str(date);
            url.push_str("/");
            url.push_str(lang);
            url.push_str("wiki-");
            url.push_str(date);
            url.push_str("-all-titles-in-ns0.gz");
            curl.arg(url);
        }
    }
    curl.output().expect("Executing curl failed");
}

fn main() {
    let mut args = std::env::args_os();
    if args.next().is_none() {
        eprintln!("Error: Program name missing from arguments.");
        std::process::exit(-1);
    }
    if let Some(command) = args.next() {
        if "check" == command {
            if let Some(label) = args.next() {
                if let Some(input) = args.next() {
                    let fast_encoder = FastEncoder::new();
                    let mut score_card = ScoreCard::new();
                    let input_string = input.to_str().unwrap();
                    let encoding = Encoding::for_label(label.to_str().unwrap().as_bytes()).unwrap();
                    let orthographic = true;
                    check(
                        input_string,
                        None,
                        encoding,
                        orthographic,
                        true,
                        &mut score_card,
                        &fast_encoder,
                        CheckMode::All,
                        0,
                        0,
                    );
                    score_card.print(input_string, encoding, true);
                } else {
                    eprintln!("Error: Test input missing.");
                    std::process::exit(-3);
                }
            } else {
                eprintln!("Error: Encoding label missing.");
                std::process::exit(-3);
            }
        } else if "download" == command {
            if let Some(path) = args.next() {
                download_titles(Path::new(&path));
            } else {
                eprintln!("Error: Download directory missing.");
                std::process::exit(-3);
            }
        } else if "all" == command
            || "tld" == command
            || "full" == command
            || "full_tld" == command
            || "all_ng" == command
            || "full_ng" == command
        {
            if let Some(dir) = args.next() {
                let max_non_ascii = if let Some(max_non_ascii_arg) = args.next() {
                    max_non_ascii_arg
                        .to_str()
                        .unwrap()
                        .parse::<usize>()
                        .unwrap()
                } else {
                    0
                };
                let mut score_card = ScoreCard::new();
                test_all(
                    Path::new(&dir),
                    false,
                    "tld" == command || "full_tld" == command,
                    &mut score_card,
                    "full" == command || "full_tld" == command || "full_ng" == command,
                    if "all" == command || "full" == command {
                        CheckMode::All
                    } else {
                        CheckMode::Ng
                    },
                    max_non_ascii,
                    0,
                );
                score_card.print("Combined", X_USER_DEFINED, true);
            } else {
                eprintln!("Error: Download directory missing.");
                std::process::exit(-3);
            }
        } else if "bench_ng" == command || "bench_ced" == command {
            if let Some(dir) = args.next() {
                let chunk = if let Some(chunk_arg) = args.next() {
                    chunk_arg.to_str().unwrap().parse::<usize>().unwrap()
                } else {
                    0
                };
                let mut score_card = ScoreCard::new();
                bench_all(
                    Path::new(&dir),
                    false,
                    false,
                    &mut score_card,
                    true,
                    if "bench_ng" == command {
                        CheckMode::Ng
                    } else {
                        CheckMode::Ced
                    },
                    0,
                    chunk,
                );
                score_card.print("Combined", X_USER_DEFINED, true);
            } else {
                eprintln!("Error: Download directory missing.");
                std::process::exit(-3);
            }
        } else if "lang" == command || "langtld" == command {
            if let Some(label) = args.next() {
                if let Some(language) = args.next() {
                    if let Some(path) = args.next() {
                        let max_non_ascii = if let Some(max_non_ascii_arg) = args.next() {
                            max_non_ascii_arg
                                .to_str()
                                .unwrap()
                                .parse::<usize>()
                                .unwrap()
                        } else {
                            0
                        };
                        let mut score_card = ScoreCard::new();
                        let language_str = language.to_str().unwrap();
                        let (lang, tld) = if "langtld" == command {
                            let mut i = language_str.len() - 1;
                            loop {
                                if language_str.as_bytes()[i] == b'-' {
                                    break;
                                }
                                i -= 1;
                            }
                            (
                                &language_str[..i],
                                Some((&language_str[i + 1..]).as_bytes()),
                            )
                        } else {
                            (language_str, None)
                        };
                        let encoding =
                            Encoding::for_label(label.to_str().unwrap().as_bytes()).unwrap();
                        let orthographic = true;
                        let fast_encoder = FastEncoder::new();
                        test_lang(
                            &find_file(Path::new(&path), lang, false),
                            tld,
                            encoding,
                            orthographic,
                            true,
                            &mut score_card,
                            &fast_encoder,
                            CheckMode::All,
                            max_non_ascii,
                            0,
                        );
                        score_card.print(lang, encoding, orthographic);
                    } else {
                        eprintln!("Error: Download directory missing.");
                        std::process::exit(-3);
                    }
                } else {
                    eprintln!("Error: Language tag missing.");
                    std::process::exit(-3);
                }
            } else {
                eprintln!("Error: Encoding label missing.");
                std::process::exit(-3);
            }
        } else {
            eprintln!("Error: Unknown command.");
            std::process::exit(-3);
        }
    } else {
        eprintln!("Error: Command missing.");
        std::process::exit(-2);
    };
}
