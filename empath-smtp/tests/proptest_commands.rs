//! Property-based tests for SMTP command parsing
//!
//! These tests use proptest to generate random valid SMTP commands and verify
//! that parsing is robust and consistent.

use empath_smtp::command::Command;
use proptest::prelude::*;

/// Strategy to generate valid domain names
fn domain_strategy() -> impl Strategy<Value = String> {
    #[allow(
        clippy::expect_used,
        reason = "compile-time constant regex should be valid"
    )]
    let regex = prop::string::string_regex("[a-z]{3,10}\\.[a-z]{2,5}")
        .expect("domain regex should be valid");
    regex.prop_map(|s| s.to_lowercase())
}

/// Strategy to generate valid email local parts according to RFC 5321
///
/// RFC 5321 Dot-string rules:
/// - Must be one or more atoms separated by dots
/// - Atom = 1\*atext (one or more valid characters)
/// - Cannot start or end with a dot
/// - Cannot have consecutive dots
/// - Valid atext chars include: alphanumeric and special characters
fn email_local_strategy() -> impl Strategy<Value = String> {
    // Generate an atom (1-10 valid atext characters)
    // Using a subset of atext that's commonly used: alphanumeric, dot, plus, underscore, hyphen
    #[allow(
        clippy::expect_used,
        reason = "compile-time constant regex should be valid"
    )]
    let atom_regex =
        prop::string::string_regex("[a-z0-9+_-]{1,10}").expect("atom regex should be valid");

    // Generate 1-3 atoms and join them with dots to create a valid Dot-string
    prop::collection::vec(atom_regex, 1..=3).prop_map(|atoms| atoms.join("."))
}

/// Strategy to generate valid email addresses
fn email_strategy() -> impl Strategy<Value = String> {
    (email_local_strategy(), domain_strategy())
        .prop_map(|(local, domain)| format!("{local}@{domain}"))
}

/// Strategy to generate simple SMTP commands (no parameters)
fn simple_command_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("QUIT".to_string()),
        Just("RSET".to_string()),
        Just("DATA".to_string()),
        Just("HELP".to_string()),
        Just("STARTTLS".to_string()),
        Just("AUTH".to_string()),
    ]
}

/// Strategy to generate HELO/EHLO commands
fn helo_command_strategy() -> impl Strategy<Value = String> {
    (prop_oneof![Just("HELO"), Just("EHLO")], domain_strategy())
        .prop_map(|(cmd, domain)| format!("{cmd} {domain}"))
}

/// Strategy to generate MAIL FROM commands
fn mail_from_strategy() -> impl Strategy<Value = String> {
    email_strategy().prop_map(|email| format!("MAIL FROM:<{email}>"))
}

/// Strategy to generate RCPT TO commands
fn rcpt_to_strategy() -> impl Strategy<Value = String> {
    email_strategy().prop_map(|email| format!("RCPT TO:<{email}>"))
}

/// Strategy to generate any valid SMTP command
fn command_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        simple_command_strategy(),
        helo_command_strategy(),
        mail_from_strategy(),
        rcpt_to_strategy(),
    ]
}

proptest! {
    /// Test that simple commands parse successfully
    #[test]
    fn test_simple_commands_parse(cmd in simple_command_strategy()) {
        let parsed = Command::try_from(cmd.as_str());
        prop_assert!(parsed.is_ok(), "Failed to parse command: {}", cmd);
    }

    /// Test that HELO/EHLO commands parse successfully
    #[test]
    fn test_helo_commands_parse(cmd in helo_command_strategy()) {
        let parsed = Command::try_from(cmd.as_str());
        prop_assert!(parsed.is_ok(), "Failed to parse HELO/EHLO: {}", cmd);
    }

    /// Test that MAIL FROM commands parse successfully
    #[test]
    fn test_mail_from_parses(cmd in mail_from_strategy()) {
        let parsed = Command::try_from(cmd.as_str());
        prop_assert!(parsed.is_ok(), "Failed to parse MAIL FROM: {}", cmd);
    }

    /// Test that RCPT TO commands parse successfully
    #[test]
    fn test_rcpt_to_parses(cmd in rcpt_to_strategy()) {
        let parsed = Command::try_from(cmd.as_str());
        prop_assert!(parsed.is_ok(), "Failed to parse RCPT TO: {}", cmd);
    }

    /// Test that parsing is case-insensitive for command keywords
    #[test]
    fn test_case_insensitive_parsing(cmd in simple_command_strategy()) {
        let lower = cmd.to_lowercase();
        let upper = cmd.to_uppercase();
        let mixed = cmd.chars().enumerate().map(|(i, c)| {
            if i % 2 == 0 {
                c.to_lowercase().to_string()
            } else {
                c.to_uppercase().to_string()
            }
        }).collect::<String>();

        let lower_result = Command::try_from(lower.as_str());
        let upper_result = Command::try_from(upper.as_str());
        let mixed_result = Command::try_from(mixed.as_str());

        prop_assert!(lower_result.is_ok(), "Failed to parse lowercase: {}", lower);
        prop_assert!(upper_result.is_ok(), "Failed to parse uppercase: {}", upper);
        prop_assert!(mixed_result.is_ok(), "Failed to parse mixed case: {}", mixed);

        // All variations should parse to the same command variant
        // SAFETY: We just checked that all results are Ok above
        #[allow(clippy::unwrap_used, reason = "checked with prop_assert above")]
        let lower_cmd = lower_result.unwrap();
        #[allow(clippy::unwrap_used, reason = "checked with prop_assert above")]
        let upper_cmd = upper_result.unwrap();
        #[allow(clippy::unwrap_used, reason = "checked with prop_assert above")]
        let mixed_cmd = mixed_result.unwrap();

        prop_assert_eq!(
            std::mem::discriminant(&lower_cmd),
            std::mem::discriminant(&upper_cmd)
        );
        prop_assert_eq!(
            std::mem::discriminant(&lower_cmd),
            std::mem::discriminant(&mixed_cmd)
        );
    }

    /// Test that invalid commands are handled gracefully
    #[test]
    fn test_invalid_commands_dont_panic(s in {
        #[allow(clippy::expect_used, reason = "compile-time constant regex should be valid")]
        let regex = prop::string::string_regex("[A-Z]{1,20}")
            .expect("invalid command regex should be valid");
        regex
    }) {
        // This test ensures we don't panic on arbitrary input
        let _ = Command::try_from(s.as_str());
        // If we get here without panicking, the test passes
    }

    /// Test that email addresses with various valid characters parse correctly
    #[test]
    fn test_email_address_characters(email in email_strategy()) {
        let mail_from = format!("MAIL FROM:<{email}>");
        let result = Command::try_from(mail_from.as_str());
        prop_assert!(result.is_ok(), "Failed to parse email: {}", email);
    }

    /// Test that commands with trailing whitespace parse correctly
    #[test]
    fn test_trailing_whitespace(cmd in command_strategy()) {
        let with_whitespace = format!("{cmd}   ");
        let result = Command::try_from(with_whitespace.as_str());
        prop_assert!(result.is_ok(), "Failed to parse command with trailing whitespace: {}", cmd);
    }

    /// Test that commands with leading whitespace parse correctly
    #[test]
    fn test_leading_whitespace(cmd in command_strategy()) {
        let with_whitespace = format!("   {cmd}");
        let result = Command::try_from(with_whitespace.as_str());
        prop_assert!(result.is_ok(), "Failed to parse command with leading whitespace: {}", cmd);
    }
}

#[cfg(test)]
mod roundtrip_tests {
    use super::*;

    proptest! {
        /// Test that simple commands roundtrip correctly (parse -> display -> parse)
        #[test]
        fn test_simple_command_roundtrip(cmd in simple_command_strategy()) {
            let parsed1_result = Command::try_from(cmd.as_str());
            prop_assert!(parsed1_result.is_ok(), "First parse failed for: {}", cmd);

            // SAFETY: We just checked that parsed1_result is Ok above
            #[allow(clippy::unwrap_used, reason = "checked with prop_assert above")]
            let parsed1 = parsed1_result.unwrap();

            let displayed = parsed1.to_string();
            let parsed2_result = Command::try_from(displayed.as_str());
            prop_assert!(parsed2_result.is_ok(), "Second parse failed for: {}", displayed);

            // SAFETY: We just checked that parsed2_result is Ok above
            #[allow(clippy::unwrap_used, reason = "checked with prop_assert above")]
            let parsed2 = parsed2_result.unwrap();

            // Both should parse to the same command variant
            prop_assert_eq!(
                std::mem::discriminant(&parsed1),
                std::mem::discriminant(&parsed2),
                "Roundtrip failed: {} -> {} -> {:?}",
                cmd,
                displayed,
                parsed2
            );
        }
    }
}
