use std::fmt::Write;

use toluca_core::GuestAddr;

use crate::pe::PeImage;
use crate::sha1::format_hex;

pub const HIGH_ENTROPY_THRESHOLD: f64 = 7.2;
pub const MIN_ENTROPY_SAMPLE_BYTES: usize = 1024;

const SAFEDISC_SECTION_PREFIXES: &[&str] = &[
    "stxt", ".icd", "clcd", ".sforce", "secdrv", ".cms_t", ".cms_d",
];
const SAFEDISC_IMPORT_DLLS: &[&str] = &["dplayerx.dll", "secdrv.sys"];
const EXPECTED_TARGET_DLLS: &[&str] = &["d3d8.dll", "dsound.dll", "dinput8.dll", "binkw32.dll"];

struct KnownProfile {
    name: &'static str,
    sha1: [u8; 20],
}

const KNOWN_PROFILES: &[KnownProfile] = &[KnownProfile {
    name: "Vanilla NA v1.0 (unprotected)",
    sha1: [
        0x25, 0x76, 0x35, 0x58, 0x80, 0x13, 0xec, 0xec, 0x81, 0xde, 0xeb, 0xae, 0x90, 0xd0, 0x98,
        0x10, 0xcb, 0x41, 0x39, 0x6b,
    ],
}];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BinaryVerdict {
    SupportedUnprotected { profile: &'static str },
    UnverifiedCleanPe { warnings: Vec<String> },
    SafeDiscDetected { reasons: Vec<String> },
    UnsupportedBinary { reason: String },
}

#[derive(Clone, Debug, PartialEq)]
pub struct VerificationReport {
    pub sha1_hex: String,
    pub entry_va: GuestAddr,
    pub text_entropy: Option<f64>,
    pub verdict: BinaryVerdict,
}

impl VerificationReport {
    pub fn is_rejected(&self) -> bool {
        matches!(
            self.verdict,
            BinaryVerdict::SafeDiscDetected { .. } | BinaryVerdict::UnsupportedBinary { .. }
        )
    }

    pub fn format_summary(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "SHA-1: {}", self.sha1_hex);
        let _ = writeln!(out, "Entry VA: 0x{:08x}", self.entry_va);
        if let Some(entropy) = self.text_entropy {
            let _ = writeln!(out, "Entry/Code entropy: {entropy:.3} bits/byte");
        }

        match &self.verdict {
            BinaryVerdict::SupportedUnprotected { profile } => {
                let _ = writeln!(out, "Verdict: SUPPORTED ({profile})");
            }
            BinaryVerdict::UnverifiedCleanPe { warnings } => {
                let _ = writeln!(out, "Verdict: CLEAN PE (unverified hash)");
                for warning in warnings {
                    let _ = writeln!(out, "  warning: {warning}");
                }
            }
            BinaryVerdict::SafeDiscDetected { reasons } => {
                let _ = writeln!(out, "Verdict: REJECTED (SafeDisc/DRM protection detected)");
                for reason in reasons {
                    let _ = writeln!(out, "  - {reason}");
                }
                let _ = writeln!(
                    out,
                    "Action required: supply an unprotected vanilla North American v1.0 sh2pc.exe."
                );
            }
            BinaryVerdict::UnsupportedBinary { reason } => {
                let _ = writeln!(out, "Verdict: UNSUPPORTED ({reason})");
            }
        }
        out
    }
}

pub fn shannon_entropy(bytes: &[u8]) -> f64 {
    if bytes.is_empty() {
        return 0.0;
    }

    let mut counts = [0u64; 256];
    for &byte in bytes {
        counts[byte as usize] += 1;
    }

    let total = bytes.len() as f64;
    let mut entropy = 0.0;
    for &count in &counts {
        if count == 0 {
            continue;
        }
        let prob = (count as f64) / total;
        entropy -= prob * prob.log2();
    }
    entropy
}

pub fn verify_pe(image: &PeImage) -> VerificationReport {
    let sha1_hex = format_hex(&image.sha1);
    let entry_va = image.entry_va();

    let entry_section = image.section_for_va(entry_va);
    let entropy_section = entry_section.or_else(|| image.section_by_name(".text"));
    let text_entropy = entropy_section.map(|sec| shannon_entropy(sec.raw_bytes(&image.raw)));

    if image.is_dll() {
        return VerificationReport {
            sha1_hex,
            entry_va,
            text_entropy,
            verdict: BinaryVerdict::UnsupportedBinary {
                reason: "target PE is a DLL; expected an x86 executable (.exe)".to_owned(),
            },
        };
    }

    let reasons = collect_safedisc_indicators(image, entry_section);
    if !reasons.is_empty() {
        return VerificationReport {
            sha1_hex,
            entry_va,
            text_entropy,
            verdict: BinaryVerdict::SafeDiscDetected { reasons },
        };
    }

    if let Some(profile) = KNOWN_PROFILES
        .iter()
        .find(|entry| entry.sha1 == image.sha1)
    {
        return VerificationReport {
            sha1_hex,
            entry_va,
            text_entropy,
            verdict: BinaryVerdict::SupportedUnprotected {
                profile: profile.name,
            },
        };
    }

    let warnings = collect_clean_pe_warnings(image);
    VerificationReport {
        sha1_hex,
        entry_va,
        text_entropy,
        verdict: BinaryVerdict::UnverifiedCleanPe { warnings },
    }
}

fn collect_safedisc_indicators(
    image: &PeImage,
    entry_section: Option<&crate::pe::PeSection>,
) -> Vec<String> {
    let mut reasons = Vec::new();

    for sec in &image.sections {
        let lower = sec.name.to_ascii_lowercase();
        if SAFEDISC_SECTION_PREFIXES
            .iter()
            .any(|prefix| lower.starts_with(prefix))
        {
            reasons.push(format!("DRM/SafeDisc section '{}' present", sec.name));
        }
    }

    if let Some(sec) = entry_section {
        let raw = sec.raw_bytes(&image.raw);
        if raw.len() >= MIN_ENTROPY_SAMPLE_BYTES {
            let entropy = shannon_entropy(raw);
            if entropy > HIGH_ENTROPY_THRESHOLD {
                reasons.push(format!(
                    "entry section '{}' has high entropy ({entropy:.2} > {HIGH_ENTROPY_THRESHOLD:.2})",
                    sec.name
                ));
            }
        }

        let has_dot_text = image
            .sections
            .iter()
            .any(|s| s.name.eq_ignore_ascii_case(".text"));
        if has_dot_text
            && !sec.name.eq_ignore_ascii_case(".text")
            && !sec.name.eq_ignore_ascii_case("CODE")
        {
            reasons.push(format!(
                "entry point lies in '{}' instead of '.text' or 'CODE'",
                sec.name
            ));
        }
    }

    for imp in &image.imports {
        let lower = imp.dll_name.to_ascii_lowercase();
        if SAFEDISC_IMPORT_DLLS.iter().any(|&drm| lower == drm) {
            reasons.push(format!("imports SafeDisc helper '{}'", imp.dll_name));
        }
    }

    reasons
}

fn collect_clean_pe_warnings(image: &PeImage) -> Vec<String> {
    let mut warnings = Vec::new();
    let missing_dlls: Vec<&str> = EXPECTED_TARGET_DLLS
        .iter()
        .copied()
        .filter(|expected| {
            !image
                .imports
                .iter()
                .any(|imp| imp.dll_name.eq_ignore_ascii_case(expected))
        })
        .collect();

    if !missing_dlls.is_empty() {
        warnings.push(format!(
            "missing expected SH2 DLL imports: {}",
            missing_dlls.join(", ")
        ));
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pe::test_helpers::{TestSectionSpec, build_test_pe32};
    use crate::pe::{IMAGE_FILE_DLL, IMAGE_SCN_CNT_CODE, IMAGE_SCN_MEM_EXECUTE, IMAGE_SCN_MEM_READ};

    #[test]
    fn shannon_entropy_bounds() {
        assert_eq!(shannon_entropy(&[]), 0.0);
        assert_eq!(shannon_entropy(&[0xAA; 256]), 0.0);

        let uniform: Vec<u8> = (0..=255).collect();
        let ent = shannon_entropy(&uniform);
        assert!((ent - 8.0).abs() < 1e-6);
    }

    #[test]
    fn rejects_dll_binaries() {
        let bytes = build_test_pe32(
            0x1000_0000,
            0x1000,
            IMAGE_FILE_DLL | 0x0102,
            &[],
            &[TestSectionSpec {
                name: ".text",
                virtual_address: 0x1000,
                characteristics: IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ,
                data: &[0xC3],
            }],
        );
        let pe = PeImage::parse(bytes).unwrap();
        let report = verify_pe(&pe);
        assert!(matches!(
            report.verdict,
            BinaryVerdict::UnsupportedBinary { .. }
        ));
        assert!(report.is_rejected());
    }

    #[test]
    fn detects_safedisc_section_and_high_entropy() {
        // Deterministic pseudo-random high-entropy payload >= 1024 bytes
        let mut high_entropy = Vec::with_capacity(2048);
        let mut state: u32 = 0x1234_5678;
        for _ in 0..2048 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            high_entropy.push((state >> 24) as u8);
        }

        let bytes = build_test_pe32(
            0x0040_0000,
            0x2000,
            0x0102,
            &[],
            &[
                TestSectionSpec {
                    name: ".text",
                    virtual_address: 0x1000,
                    characteristics: IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ,
                    data: &[0x90; 64],
                },
                TestSectionSpec {
                    name: "stxt371",
                    virtual_address: 0x2000,
                    characteristics: IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ,
                    data: &high_entropy,
                },
            ],
        );

        let pe = PeImage::parse(bytes).unwrap();
        let report = verify_pe(&pe);
        assert!(report.is_rejected());
        match &report.verdict {
            BinaryVerdict::SafeDiscDetected { reasons } => {
                assert!(reasons.iter().any(|r| r.contains("stxt371")));
                assert!(reasons.iter().any(|r| r.contains("high entropy")));
                assert!(reasons.iter().any(|r| r.contains("entry point lies in")));
            }
            other => panic!("expected SafeDiscDetected, got {other:?}"),
        }
    }

    #[test]
    fn clean_pe_produces_unverified_verdict() {
        let bytes = build_test_pe32(
            0x0040_0000,
            0x1000,
            0x0102,
            &[],
            &[TestSectionSpec {
                name: ".text",
                virtual_address: 0x1000,
                characteristics: IMAGE_SCN_CNT_CODE | IMAGE_SCN_MEM_EXECUTE | IMAGE_SCN_MEM_READ,
                data: &[0x55, 0x8B, 0xEC, 0x5D, 0xC3],
            }],
        );
        let pe = PeImage::parse(bytes).unwrap();
        let report = verify_pe(&pe);
        assert!(!report.is_rejected());
        assert!(matches!(
            report.verdict,
            BinaryVerdict::UnverifiedCleanPe { .. }
        ));
    }
}
