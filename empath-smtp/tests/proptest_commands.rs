//! Property-based tests for SMTP command parsing
//!
//! These tests use proptest to generate random valid SMTP commands and verify
//! that parsing is robust and consistent.

use empath_smtp::command::Command;
use proptest::prelude::*;
use proptest::test_runner::TestRunner;

/// Strategy to generate valid domain names
fn domain_strategy() -> BoxedStrategy<String> {
    regex_strategy("[a-z]{3,10}\\.[a-z]{2,5}", "example.com")
        .prop_map(|s| s.to_lowercase())
        .boxed()
}

/// Strategy to generate valid email local parts according to RFC 5321
///
/// RFC 5321 Dot-string rules:
/// - Must be one or more atoms separated by dots
/// - Atom = 1\*atext (one or more valid characters)
/// - Cannot start or end with a dot
/// - Cannot have consecutive dots
/// - Valid atext chars include: alphanumeric and special characters
fn email_local_strategy() -> BoxedStrategy<String> {
    // Generate an atom (1-10 valid atext characters)
    // Using a subset of atext that's commonly used: alphanumeric, dot, plus, underscore, hyphen
    let atom_regex = regex_strategy("[a-z0-9+_-]{1,10}", "local");

    // Generate 1-3 atoms and join them with dots to create a valid Dot-string
    prop::collection::vec(atom_regex, 1..=3)
        .prop_map(|atoms| atoms.join("."))
        .boxed()
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
        Just("TURN".to_string()),
    ]
}

/// Strategy to generate AUTH commands with a mechanism
fn auth_command_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        Just("AUTH PLAIN".to_string()),
        Just("AUTH LOGIN".to_string()),
        Just("AUTH PLAIN dGVzdA==".to_string()),
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

/// Strategy to generate MAIL FROM commands with SIZE parameter and optional SMTPUTF8 flag
fn mail_from_with_size_strategy() -> impl Strategy<Value = (String, usize, bool)> {
    (email_strategy(), 1usize..=1_000_000, proptest::bool::ANY)
}

/// Strategy to generate RCPT TO commands
fn rcpt_to_strategy() -> impl Strategy<Value = String> {
    email_strategy().prop_map(|email| format!("RCPT TO:<{email}>"))
}

fn arg_strategy() -> impl Strategy<Value = String> {
    regex_strategy("[A-Za-z0-9._-]{1,20}", "arg")
}

fn noop_command_strategy() -> impl Strategy<Value = (String, Option<String>)> {
    prop_oneof![
        Just(("NOOP".to_string(), None)),
        arg_strategy().prop_map(|arg| (format!("NOOP {arg}"), Some(arg))),
    ]
}

fn vrfy_command_strategy() -> impl Strategy<Value = (String, String)> {
    arg_strategy().prop_map(|arg| (format!("VRFY {arg}"), arg))
}

fn expn_command_strategy() -> impl Strategy<Value = (String, String)> {
    arg_strategy().prop_map(|arg| (format!("EXPN {arg}"), arg))
}

fn etrn_command_strategy() -> impl Strategy<Value = (String, String)> {
    arg_strategy().prop_map(|arg| (format!("ETRN {arg}"), arg))
}

fn xclient_strategy() -> impl Strategy<Value = (String, Vec<(String, String)>)> {
    let key_strategy = prop_oneof![
        Just("ADDR".to_string()),
        Just("NAME".to_string()),
        Just("HELO".to_string()),
        Just("PROTO".to_string()),
        Just("LOGIN".to_string()),
    ];
    let value_strategy = regex_strategy("[A-Za-z0-9._:-]{1,20}", "value");
    prop::collection::hash_map(key_strategy, value_strategy, 1..=5).prop_map(|map| {
        let mut pairs: Vec<(String, String)> = map.into_iter().collect();
        pairs.sort_by(|left, right| left.0.cmp(&right.0));
        let params = pairs
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(" ");
        let command = format!("XCLIENT {params}");
        (command, pairs)
    })
}

fn invalid_command_strategy() -> BoxedStrategy<String> {
    regex_strategy("[A-Z]{1,20}", "INVALID")
}

fn regex_strategy(pattern: &str, fallback: &str) -> BoxedStrategy<String> {
    prop::string::string_regex(pattern).map_or_else(
        |_| Just(fallback.to_string()).boxed(),
        proptest::strategy::Strategy::boxed,
    )
}

/// Strategy to generate any valid SMTP command
fn command_strategy() -> impl Strategy<Value = String> {
    prop_oneof![
        simple_command_strategy(),
        auth_command_strategy(),
        helo_command_strategy(),
        mail_from_strategy(),
        rcpt_to_strategy(),
        noop_command_strategy().prop_map(|(cmd, _)| cmd),
        vrfy_command_strategy().prop_map(|(cmd, _)| cmd),
        expn_command_strategy().prop_map(|(cmd, _)| cmd),
        etrn_command_strategy().prop_map(|(cmd, _)| cmd),
        xclient_strategy().prop_map(|(cmd, _)| cmd),
    ]
}

proptest! {
    /// Test that simple commands parse successfully
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_simple_commands_parse(cmd in simple_command_strategy()) {
        let parsed = Command::try_from(cmd.as_str());
        prop_assert!(parsed.is_ok(), "Failed to parse command: {}", cmd);
    }

    /// Test that HELO/EHLO commands parse successfully
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_helo_commands_parse(cmd in helo_command_strategy()) {
        let parsed = Command::try_from(cmd.as_str());
        prop_assert!(parsed.is_ok(), "Failed to parse HELO/EHLO: {}", cmd);
    }

    /// Test that MAIL FROM commands parse successfully
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_mail_from_parses(cmd in mail_from_strategy()) {
        let parsed = Command::try_from(cmd.as_str());
        prop_assert!(parsed.is_ok(), "Failed to parse MAIL FROM: {}", cmd);
    }

    /// Test that MAIL FROM with SIZE parameter preserves declared size and flags
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_mail_from_size_and_flags((email, size, smtp_utf8) in mail_from_with_size_strategy()) {
        let command = if smtp_utf8 {
            format!("MAIL FROM:<{email}> SIZE={size} SMTPUTF8")
        } else {
            format!("MAIL FROM:<{email}> SIZE={size}")
        };

        let parsed = Command::try_from(command.as_str());
        let params = match parsed {
            Ok(Command::MailFrom(_, params)) => params,
            Ok(other) => {
                return Err(TestCaseError::fail(format!(
                    "Unexpected command variant for {command}: {other:?}"
                )));
            }
            Err(err) => {
                return Err(TestCaseError::fail(format!(
                    "Failed to parse MAIL FROM with SIZE: {command}: {err:?}"
                )));
            }
        };

        prop_assert_eq!(params.size(), Some(size));
        prop_assert_eq!(params.has("SMTPUTF8"), smtp_utf8);
    }

    /// Test that RCPT TO commands parse successfully
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_rcpt_to_parses(cmd in rcpt_to_strategy()) {
        let parsed = Command::try_from(cmd.as_str());
        prop_assert!(parsed.is_ok(), "Failed to parse RCPT TO: {}", cmd);
    }

    /// Test that NOOP commands preserve optional arguments
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_noop_parses((command, expected_arg) in noop_command_strategy()) {
        let parsed = Command::try_from(command.as_str());
        let arg = match parsed {
            Ok(Command::Noop(arg)) => arg,
            Ok(other) => {
                return Err(TestCaseError::fail(format!(
                    "Unexpected command variant for {command}: {other:?}"
                )));
            }
            Err(err) => {
                return Err(TestCaseError::fail(format!(
                    "Failed to parse NOOP: {command}: {err:?}"
                )));
            }
        };

        prop_assert_eq!(arg, expected_arg);
    }

    /// Test that VRFY commands capture the argument
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_vrfy_parses((command, expected) in vrfy_command_strategy()) {
        let parsed = Command::try_from(command.as_str());
        let arg = match parsed {
            Ok(Command::Vrfy(arg)) => arg,
            Ok(other) => {
                return Err(TestCaseError::fail(format!(
                    "Unexpected command variant for {command}: {other:?}"
                )));
            }
            Err(err) => {
                return Err(TestCaseError::fail(format!(
                    "Failed to parse VRFY: {command}: {err:?}"
                )));
            }
        };

        prop_assert_eq!(arg, expected);
    }

    /// Test that EXPN commands capture the argument
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_expn_parses((command, expected) in expn_command_strategy()) {
        let parsed = Command::try_from(command.as_str());
        let arg = match parsed {
            Ok(Command::Expn(arg)) => arg,
            Ok(other) => {
                return Err(TestCaseError::fail(format!(
                    "Unexpected command variant for {command}: {other:?}"
                )));
            }
            Err(err) => {
                return Err(TestCaseError::fail(format!(
                    "Failed to parse EXPN: {command}: {err:?}"
                )));
            }
        };

        prop_assert_eq!(arg, expected);
    }

    /// Test that ETRN commands capture the argument
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_etrn_parses((command, expected) in etrn_command_strategy()) {
        let parsed = Command::try_from(command.as_str());
        let arg = match parsed {
            Ok(Command::Etrn(arg)) => arg,
            Ok(other) => {
                return Err(TestCaseError::fail(format!(
                    "Unexpected command variant for {command}: {other:?}"
                )));
            }
            Err(err) => {
                return Err(TestCaseError::fail(format!(
                    "Failed to parse ETRN: {command}: {err:?}"
                )));
            }
        };

        prop_assert_eq!(arg, expected);
    }

    /// Test that XCLIENT parameters are parsed and stored correctly
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_xclient_parses((command, expected) in xclient_strategy()) {
        let parsed = Command::try_from(command.as_str());
        let params = match parsed {
            Ok(Command::Xclient(params)) => params,
            Ok(other) => {
                return Err(TestCaseError::fail(format!(
                    "Unexpected command variant for {command}: {other:?}"
                )));
            }
            Err(err) => {
                return Err(TestCaseError::fail(format!(
                    "Failed to parse XCLIENT: {command}: {err:?}"
                )));
            }
        };

        for (key, value) in expected {
            prop_assert_eq!(params.get(&key), Some(value.as_str()));
        }
    }

    /// Test that parsing is case-insensitive for command keywords
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
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

        let lower_cmd = match Command::try_from(lower.as_str()) {
            Ok(cmd) => cmd,
            Err(err) => {
                return Err(TestCaseError::fail(format!(
                    "Failed to parse lowercase: {lower}: {err:?}"
                )));
            }
        };
        let upper_cmd = match Command::try_from(upper.as_str()) {
            Ok(cmd) => cmd,
            Err(err) => {
                return Err(TestCaseError::fail(format!(
                    "Failed to parse uppercase: {upper}: {err:?}"
                )));
            }
        };
        let mixed_cmd = match Command::try_from(mixed.as_str()) {
            Ok(cmd) => cmd,
            Err(err) => {
                return Err(TestCaseError::fail(format!(
                    "Failed to parse mixed case: {mixed}: {err:?}"
                )));
            }
        };

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
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_invalid_commands_dont_panic(s in invalid_command_strategy()) {
        // This test ensures we don't panic on arbitrary input
        let _ = Command::try_from(s.as_str());
        // If we get here without panicking, the test passes
    }

    /// Test that email addresses with various valid characters parse correctly
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_email_address_characters(email in email_strategy()) {
        let mail_from = format!("MAIL FROM:<{email}>");
        let result = Command::try_from(mail_from.as_str());
        prop_assert!(result.is_ok(), "Failed to parse email: {}", email);
    }

    /// Test that commands with trailing whitespace parse correctly
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_trailing_whitespace(cmd in command_strategy()) {
        let with_whitespace = format!("{cmd}   ");
        let result = Command::try_from(with_whitespace.as_str());
        prop_assert!(result.is_ok(), "Failed to parse command with trailing whitespace: {}", cmd);
    }

    /// Test that non-numeric SIZE values are rejected
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_invalid_size_values(email in email_strategy(), size in regex_strategy("[A-Za-z]{1,6}", "BAD")) {
        let command = format!("MAIL FROM:<{email}> SIZE={size}");
        let parsed = Command::try_from(command.as_str());
        prop_assert!(parsed.is_err(), "Expected SIZE parse failure for: {}", command);
    }

    /// Test that commands with leading whitespace parse correctly
    #[test]
    #[cfg_attr(miri, ignore = "Calls an unsupported method")]
    fn test_leading_whitespace(cmd in command_strategy()) {
        let with_whitespace = format!("   {cmd}");
        let result = Command::try_from(with_whitespace.as_str());
        prop_assert!(result.is_ok(), "Failed to parse command with leading whitespace: {}", cmd);
    }
}

#[test]
fn test_regex_strategy_fallback_uses_fallback() -> Result<(), String> {
    let mut runner = TestRunner::default();
    let tree = regex_strategy("(", "fallback")
        .new_tree(&mut runner)
        .map_err(|err| err.to_string())?;
    assert_eq!(tree.current(), "fallback");
    Ok(())
}

#[cfg(test)]
mod roundtrip_tests {
    use super::*;

    proptest! {
        /// Test that simple commands roundtrip correctly (parse -> display -> parse)
        #[test]
        #[cfg_attr(miri, ignore = "Calls an unsupported method")]
        fn test_simple_command_roundtrip(cmd in simple_command_strategy()) {
            let parsed1 = match Command::try_from(cmd.as_str()) {
                Ok(cmd) => cmd,
                Err(err) => {
                    return Err(TestCaseError::fail(format!(
                        "First parse failed for {cmd}: {err:?}"
                    )));
                }
            };

            let displayed = parsed1.to_string();
            let parsed2 = match Command::try_from(displayed.as_str()) {
                Ok(cmd) => cmd,
                Err(err) => {
                    return Err(TestCaseError::fail(format!(
                        "Second parse failed for {displayed}: {err:?}"
                    )));
                }
            };

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
