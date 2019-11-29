// Copyright 2019 Mozilla Foundation. See the COPYRIGHT
// file at the top-level directory of this distribution.
//
// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// https://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or https://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use encoding_rs::BIG5;
use encoding_rs::EUC_KR;
use encoding_rs::GBK;
use encoding_rs::BIG5_INIT;
use encoding_rs::GBK_INIT;
use encoding_rs::DecoderResult;
use encoding_rs::EUC_JP_INIT;
use encoding_rs::EUC_KR_INIT;
use encoding_rs::ISO_8859_13_INIT;
use encoding_rs::SHIFT_JIS_INIT;
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

fn find_file(dir: &Path, lang: &str) -> PathBuf {
    for entry in dir.read_dir().expect("Reading the title directory failed.") {
        if let Ok(entry) = entry {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if s.starts_with(lang) && s.ends_with(".gz") {
                return entry.path();
            }
        }
    }
    eprintln!("Error: No titles for: {}", lang);
    std::process::exit(-4);
}

fn test_lang(
    path: &Path,
    enc: &'static Encoding,
    orthographic: bool,
    print: bool,
    score_card: &mut ScoreCard,
    fast_encoder: &FastEncoder,
) {
    let media_wiki_special =
        Regex::new(r"^(?:\u{200D}\u{200C})?\p{Alphabetic}+:\p{Alphabetic}+$").unwrap();
    let mut read = BufReader::new(Decoder::new(BufReader::new(File::open(path).unwrap())).unwrap());
    loop {
        let mut buf = String::new();
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
        check(s, enc, orthographic, print, score_card, &fast_encoder);
    }
}

#[derive(Debug)]
struct EncodingClass {
    encodings: &'static [&'static Encoding],
    languages: &'static [&'static str],
    name: &'static str,
}

static ENCODING_CLASSES: [EncodingClass; 14] = [
    // Vietnamese consumes the corpus twice, so put it first
    // to maximize parallelism.
    // In the `encodings` field, the Windows encoding comes first.
    EncodingClass {
        encodings: &[&WINDOWS_1258_INIT],
        languages: &["vi"],
        name: "vietnamese",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1250_INIT, &ISO_8859_2_INIT],
        languages: &["pl", "hu", "sh", "cs", "ro", "sk", "hr", "sl", "bs", "sq"],
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
        languages: &["ru", "uk", "sr", "bg", "ce", "be", "mk", "mn"],
        name: "cyrillic",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1252_INIT],
        // Intentionally omitting ASCII languages like en, nl, id, so, sw, various Malay-alphabet languages
        languages: &[
            "sv", "de", "fr", "it", "es", "pt", "ca", "no", "fi", "eu", "da", "gl", "nn", "oc",
            "br", "lb", "ht", "ga", "is", "an", "wa", "gd", "fo", "li",
        ],
        name: "western",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1253_INIT, &ISO_8859_7_INIT],
        languages: &["el"],
        name: "greek",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1254_INIT],
        languages: &["tr", "az", "ku"],
        name: "turkish",
    },
    EncodingClass {
        encodings: &[
            &WINDOWS_1255_INIT, // , &ISO_8859_8_INIT
        ],
        languages: &["he", "yi"],
        name: "hebrew",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1256_INIT, &ISO_8859_6_INIT],
        languages: &["ar", "fa", "ur"],
        name: "arabic",
    },
    EncodingClass {
        encodings: &[&WINDOWS_1257_INIT, &ISO_8859_4_INIT],
        languages: &["lt", "et", "lv"],
        name: "baltic",
    },
    EncodingClass {
        encodings: &[&WINDOWS_874_INIT],
        languages: &["th"],
        name: "thai",
    },
    EncodingClass {
        encodings: &[&SHIFT_JIS_INIT, &EUC_JP_INIT],
        languages: &["ja"],
        name: "japanese",
    },
    EncodingClass {
        encodings: &[&EUC_KR_INIT],
        languages: &["ko"],
        name: "korean",
    },
    EncodingClass {
        encodings: &[&GBK_INIT],
        languages: &["zh-hans"],
        name: "simplified",
    },
    EncodingClass {
        encodings: &[&BIG5_INIT],
        languages: &["zh-hant"],
        name: "traditional",
    },
];

fn test_one(
    lang: &str,
    dir: &Path,
    enc: &'static Encoding,
    orthographic: bool,
    print: bool,
    fast_encoder: &FastEncoder,
) -> ScoreCard {
    let title_path = find_file(dir, lang);
    let mut score_card = ScoreCard::new();
    test_lang(
        &title_path,
        enc,
        orthographic,
        print,
        &mut score_card,
        fast_encoder,
    );
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

fn ng(buffer: &[u8], det: &mut EncodingDetector) -> &'static Encoding {
    det.feed(buffer, true);
    det.guess(None, false)
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
            .decompose_vietnamese_tones(orthographic).map(|c| match c {
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

fn check(
    s: &str,
    encoding: &'static Encoding,
    orthographic: bool,
    print: bool,
    score_card: &mut ScoreCard,
    fast_encoder: &FastEncoder,
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
        let chardet = check_chardet(encoding, &bytes);
        let ced = check_ced(encoding, &bytes);
        let icu = check_icu(encoding, &bytes);

        score_card.total += 1;
        score_card.chardet += chardet as u64;
        score_card.ced += ced as u64;
        score_card.icu += icu as u64;

        if let Some((
            detected,
            actual_text,
            detected_score,
            expected_text,
            expected_score,
            expected_disqualified,
        )) = check_ng(encoding, &bytes)
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

fn test_all(dir: &Path, print: bool, total_scores: &mut ScoreCard) {
    let fast_encoder = FastEncoder::new();
    // There are likely fancy iterator tricks for this.
    let mut tasks = Vec::new();
    for encoding_class in ENCODING_CLASSES.iter() {
        for &lang in encoding_class.languages.iter() {
            for &encoding in encoding_class.encodings.iter() {
                tasks.push((lang, encoding, false));
                if encoding == WINDOWS_1258 {
                    tasks.push((lang, encoding, true));
                }
            }
        }
    }
    let score_cards: Vec<ScoreCard> = tasks
        .par_iter()
        .map(|&task| {
            let (lang, encoding, orthographic) = task;
            let score_card = test_one(lang, dir, encoding, orthographic, print, &fast_encoder);
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
        for lang in encoding_class.languages.iter() {
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
                    let lang = input.to_str().unwrap();
                    let encoding = Encoding::for_label(label.to_str().unwrap().as_bytes()).unwrap();
                    let orthographic = true;
                    check(
                        lang,
                        encoding,
                        orthographic,
                        true,
                        &mut score_card,
                        &fast_encoder,
                    );
                    score_card.print(lang, encoding, true);
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
        } else if "all" == command {
            if let Some(dir) = args.next() {
                let mut score_card = ScoreCard::new();
                test_all(Path::new(&dir), false, &mut score_card);
                score_card.print("Combined", X_USER_DEFINED, true);
            } else {
                eprintln!("Error: Download directory missing.");
                std::process::exit(-3);
            }
        } else if "lang" == command {
            if let Some(label) = args.next() {
                if let Some(language) = args.next() {
                    if let Some(path) = args.next() {
                        let mut score_card = ScoreCard::new();
                        let lang = language.to_str().unwrap();
                        let encoding =
                            Encoding::for_label(label.to_str().unwrap().as_bytes()).unwrap();
                        let orthographic = true;
                        let fast_encoder = FastEncoder::new();
                        test_lang(
                            &find_file(Path::new(&path), lang),
                            encoding,
                            orthographic,
                            true,
                            &mut score_card,
                            &fast_encoder,
                        );
                        score_card.print(lang, encoding, orthographic);
                    } else {
                        eprintln!("Error: Downoald directory missing.");
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
