//! Benchmarks for SMTP protocol implementation
//!
//! This benchmark suite tests the performance of critical SMTP operations:
//! - Command parsing from strings and bytes
//! - ESMTP parameter parsing with perfect hash map
//! - FSM state transitions
//! - Context operations
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{hint::black_box, sync::Arc};

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use empath_common::{address::Address, address_parser, context::Context};
use empath_smtp::{
    MailParameters, State,
    command::{Command, HeloVariant},
    state::{Connect, Data, Ehlo, MailFrom, RcptTo},
};

// ============================================================================
// Command Parsing Benchmarks
// ============================================================================

fn bench_command_parsing(c: &mut Criterion) {
    let mut group = c.benchmark_group("command_parsing");

    // Test various command types with different complexities
    let commands = vec![
        ("QUIT", "Simple QUIT command"),
        ("DATA", "Simple DATA command"),
        ("HELP", "Simple HELP command"),
        ("RSET", "Simple RSET command"),
        ("STARTTLS", "STARTTLS command"),
        ("HELO example.com", "HELO with hostname"),
        ("EHLO mail.example.com", "EHLO with hostname"),
        (
            "MAIL FROM:<user@example.com>",
            "MAIL FROM without parameters",
        ),
        (
            "MAIL FROM:<user@example.com> SIZE=12345",
            "MAIL FROM with SIZE",
        ),
        (
            "MAIL FROM:<user@example.com> SIZE=12345 BODY=8BITMIME",
            "MAIL FROM with multiple params",
        ),
        (
            "MAIL FROM:<user@example.com> SIZE=12345 BODY=8BITMIME AUTH=<> SMTPUTF8",
            "MAIL FROM with many params",
        ),
        ("MAIL FROM:<>", "MAIL FROM null sender"),
        ("RCPT TO:<recipient@example.com>", "RCPT TO single address"),
        (
            "RCPT TO:<recipient@example.com>, <another@example.com>",
            "RCPT TO multiple addresses",
        ),
    ];

    for (cmd, desc) in commands {
        group.throughput(Throughput::Bytes(cmd.len() as u64));
        group.bench_with_input(BenchmarkId::new("from_str", desc), &cmd, |b, &cmd| {
            b.iter(|| {
                let result: Result<Command, Command> = black_box(cmd).try_into();
                black_box(result)
            });
        });
    }

    group.finish();
}

fn bench_command_parsing_bytes(c: &mut Criterion) {
    let mut group = c.benchmark_group("command_parsing_bytes");

    let commands = vec![
        (b"QUIT" as &[u8], "QUIT"),
        (b"EHLO example.com", "EHLO"),
        (b"MAIL FROM:<user@example.com> SIZE=12345", "MAIL FROM"),
    ];

    for (cmd_bytes, desc) in commands {
        group.throughput(Throughput::Bytes(cmd_bytes.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(desc),
            &cmd_bytes,
            |b, &bytes| {
                b.iter(|| {
                    let result: Result<Command, Command> = black_box(bytes).try_into();
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

// ============================================================================
// MailParameters Parsing Benchmarks
// ============================================================================

fn bench_mail_parameters(c: &mut Criterion) {
    let mut group = c.benchmark_group("mail_parameters");

    let param_strings = vec![
        ("SIZE=12345", "single_param"),
        ("SIZE=12345 BODY=8BITMIME", "two_params"),
        ("SIZE=12345 BODY=8BITMIME AUTH=<>", "three_params"),
        (
            "SIZE=12345 BODY=8BITMIME AUTH=<> RET=FULL ENVID=xyz SMTPUTF8",
            "many_params",
        ),
        ("SMTPUTF8", "flag_only"),
        ("size=12345 body=8bitmime", "lowercase_params"),
    ];

    for (params_str, desc) in param_strings {
        group.throughput(Throughput::Bytes(params_str.len() as u64));
        group.bench_with_input(
            BenchmarkId::from_parameter(desc),
            &params_str,
            |b, params_str| {
                b.iter(|| {
                    let result = MailParameters::from_params_str(black_box(params_str));
                    black_box(result)
                });
            },
        );
    }

    group.finish();
}

fn bench_mail_parameters_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("mail_parameters_operations");

    // Benchmark parameter lookups (tests perfect hash map performance)
    let params = MailParameters::from_params_str("SIZE=12345 BODY=8BITMIME AUTH=<> SMTPUTF8")
        .expect("Valid params");

    group.bench_function("has_known_param", |b| {
        b.iter(|| black_box(&params).has(black_box("SIZE")));
    });

    group.bench_function("has_unknown_param", |b| {
        b.iter(|| black_box(&params).has(black_box("CUSTOM")));
    });

    group.bench_function("get_existing", |b| {
        b.iter(|| black_box(&params).get(black_box("SIZE")));
    });

    group.bench_function("get_missing", |b| {
        b.iter(|| black_box(&params).get(black_box("MISSING")));
    });

    group.bench_function("size_extraction", |b| {
        b.iter(|| black_box(&params).size());
    });

    group.finish();
}

// ============================================================================
// FSM State Transition Benchmarks
// ============================================================================

fn bench_fsm_transitions(c: &mut Criterion) {
    let mut group = c.benchmark_group("fsm_transitions");

    // Simple transitions that don't require complex commands
    group.bench_function("Connect -> Ehlo", |b| {
        b.iter(|| {
            let mut ctx = Context::default();
            let state = State::Connect(Connect);
            let cmd = Command::Helo(HeloVariant::Ehlo("example.com".to_string()));
            let new_state = black_box(state).transition(black_box(cmd), &mut ctx);
            black_box(new_state)
        });
    });

    group.bench_function("Connect -> Helo", |b| {
        b.iter(|| {
            let mut ctx = Context::default();
            let state = State::Connect(Connect);
            let cmd = Command::Helo(HeloVariant::Helo("example.com".to_string()));
            let new_state = black_box(state).transition(black_box(cmd), &mut ctx);
            black_box(new_state)
        });
    });

    group.bench_function("Ehlo -> MailFrom", |b| {
        b.iter(|| {
            let mut ctx = Context::default();
            let state = State::Ehlo(Ehlo {
                id: "example.com".to_string(),
            });
            let cmd = Command::MailFrom(
                Some(
                    address_parser::parse_forward_path("<user@example.com>")
                        .expect("Valid address")
                        .into(),
                ),
                MailParameters::new(),
            );
            let new_state = black_box(state).transition(black_box(cmd), &mut ctx);
            black_box(new_state)
        });
    });

    group.bench_function("MailFrom -> RcptTo", |b| {
        b.iter(|| {
            let mut ctx = Context::default();
            let state = State::MailFrom(MailFrom {
                sender: Some(
                    address_parser::parse_forward_path("<user@example.com>")
                        .expect("Valid address")
                        .into(),
                ),
                params: MailParameters::new(),
            });
            let cmd = Command::RcptTo(
                vec![Address::from(
                    address_parser::parse_forward_path("<recipient@example.com>")
                        .expect("Valid address"),
                )]
                .into(),
            );
            let new_state = black_box(state).transition(black_box(cmd), &mut ctx);
            black_box(new_state)
        });
    });

    group.bench_function("RcptTo -> Data", |b| {
        b.iter(|| {
            let mut ctx = Context::default();
            let state = State::RcptTo(RcptTo {
                sender: Some(
                    address_parser::parse_forward_path("<user@example.com>")
                        .expect("Valid address")
                        .into(),
                ),
                params: MailParameters::new(),
            });
            let cmd = Command::Data;
            let new_state = black_box(state).transition(black_box(cmd), &mut ctx);
            black_box(new_state)
        });
    });

    group.bench_function("Data -> Quit", |b| {
        b.iter(|| {
            let mut ctx = Context::default();
            let state = State::Data(Data);
            let cmd = Command::Quit;
            let new_state = black_box(state).transition(black_box(cmd), &mut ctx);
            black_box(new_state)
        });
    });

    group.finish();
}

fn bench_fsm_full_transaction(c: &mut Criterion) {
    let mut group = c.benchmark_group("fsm_full_transaction");

    // Benchmark a complete SMTP transaction sequence
    group.bench_function("complete_transaction", |b| {
        b.iter(|| {
            let mut ctx = Context::default();
            let mut state = State::Connect(Connect);

            // EHLO
            state = state.transition(
                Command::Helo(HeloVariant::Ehlo("example.com".to_string())),
                &mut ctx,
            );

            // MAIL FROM
            state = state.transition(
                Command::MailFrom(
                    Some(
                        address_parser::parse_forward_path("<sender@example.com>")
                            .expect("Valid address")
                            .into(),
                    ),
                    MailParameters::new(),
                ),
                &mut ctx,
            );

            // RCPT TO
            state = state.transition(
                Command::RcptTo(
                    vec![Address::from(
                        address_parser::parse_forward_path("<recipient@example.com>")
                            .expect("Valid address"),
                    )]
                    .into(),
                ),
                &mut ctx,
            );

            // DATA
            state = state.transition(Command::Data, &mut ctx);

            // QUIT
            state = state.transition(Command::Quit, &mut ctx);

            black_box(state)
        });
    });

    group.finish();
}

// ============================================================================
// Context Operations Benchmarks
// ============================================================================

fn bench_context_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("context_operations");

    // Test context creation (relevant to recent optimization reducing clone usage)
    group.bench_function("context_default", |b| {
        b.iter(|| {
            let ctx = Context::default();
            black_box(ctx)
        });
    });

    group.bench_function("context_with_data", |b| {
        b.iter(|| {
            let mut envelope = empath_common::envelope::Envelope::default();
            *envelope.sender_mut() = Some(
                address_parser::parse_forward_path("<sender@example.com>")
                    .expect("Valid address")
                    .into(),
            );
            *envelope.recipients_mut() = Some(
                vec![Address::from(
                    address_parser::parse_forward_path("<recipient@example.com>")
                        .expect("Valid address"),
                )]
                .into(),
            );

            let ctx = Context {
                id: "example.com".to_string(),
                envelope,
                data: Some(Arc::from(
                    b"Subject: Test\r\n\r\nTest message body" as &[u8],
                )),
                ..Context::default()
            };
            black_box(ctx)
        });
    });

    group.finish();
}

// ============================================================================
// Criterion Configuration
// ============================================================================

criterion_group!(
    benches,
    bench_command_parsing,
    bench_command_parsing_bytes,
    bench_mail_parameters,
    bench_mail_parameters_operations,
    bench_fsm_transitions,
    bench_fsm_full_transaction,
    bench_context_operations,
);
criterion_main!(benches);
