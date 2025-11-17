//! Benchmarks for spool operations
//!
//! This benchmark suite tests the performance of message spooling operations:
//! - Message creation and builder pattern
//! - Bincode serialization/deserialization of metadata
//! - ULID generation and parsing
//! - In-memory spool operations (write, read, list, delete)
#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{borrow::Cow, hint::black_box, sync::Arc};

use ahash::AHashMap;
use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use empath_common::{address::Address, address_parser, context::Context, envelope::Envelope};
use empath_spool::{BackingStore, MemoryBackingStore, SpooledMessageId};

// ============================================================================
// Message Creation Benchmarks
// ============================================================================

fn create_test_message(data_size: usize) -> Context {
    let data = vec![b'X'; data_size];
    let mut context = AHashMap::new();
    context.insert(Cow::Borrowed("protocol"), "SMTP".to_string());
    context.insert(Cow::Borrowed("tls_version"), "TLSv1.3".to_string());

    let mut envelope = Envelope::default();
    *envelope.sender_mut() = Some(
        address_parser::parse_forward_path("<sender@example.com>")
            .expect("Valid address")
            .into(),
    );
    *envelope.recipients_mut() = Some(
        vec![Address::from(
            address_parser::parse_forward_path("<recipient@example.com>").expect("Valid address"),
        )]
        .into(),
    );

    Context {
        envelope,
        data: Some(Arc::from(data.into_boxed_slice())),
        extended: true,
        metadata: context,
        ..Default::default()
    }
}

fn bench_message_creation(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_creation");

    let sizes = vec![
        (1024, "1KB"),
        (10 * 1024, "10KB"),
        (100 * 1024, "100KB"),
        (1024 * 1024, "1MB"),
    ];

    for (size, desc) in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(desc), &size, |b, &size| {
            b.iter(|| {
                let msg = create_test_message(black_box(size));
                black_box(msg)
            });
        });
    }

    group.finish();
}

// ============================================================================
// SpooledMessageId Benchmarks
// ============================================================================

fn bench_message_id_operations(c: &mut Criterion) {
    let mut group = c.benchmark_group("message_id_operations");

    group.bench_function("generate_ulid", |b| {
        b.iter(|| {
            let id = SpooledMessageId::generate();
            black_box(id)
        });
    });

    group.bench_function("from_filename_valid", |b| {
        b.iter(|| {
            let id = SpooledMessageId::from_filename(black_box("01ARYZ6S41TST000000000.bin"));
            black_box(id)
        });
    });

    group.bench_function("from_filename_invalid_path", |b| {
        b.iter(|| {
            let id = SpooledMessageId::from_filename(black_box("../01ARYZ6S41TST000000000.bin"));
            black_box(id)
        });
    });

    let id = SpooledMessageId::generate();
    group.bench_function("to_string", |b| {
        b.iter(|| {
            let s = black_box(&id).to_string();
            black_box(s)
        });
    });

    group.bench_function("timestamp_ms", |b| {
        b.iter(|| {
            let ts = black_box(&id).timestamp_ms();
            black_box(ts)
        });
    });

    group.finish();
}

// ============================================================================
// In-Memory Spool Operations Benchmarks
// ============================================================================

fn bench_spool_write(c: &mut Criterion) {
    let mut group = c.benchmark_group("spool_write");

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    let sizes = vec![(1024, "1KB"), (10 * 1024, "10KB"), (100 * 1024, "100KB")];

    for (size, desc) in sizes {
        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(desc), &size, |b, &size| {
            b.to_async(&runtime).iter(|| async move {
                let spool = MemoryBackingStore::new();
                let mut message = create_test_message(black_box(size));
                let id = spool.write(&mut message).await.expect("Write succeeds");
                black_box(id)
            });
        });
    }

    group.finish();
}

fn bench_spool_read(c: &mut Criterion) {
    let mut group = c.benchmark_group("spool_read");

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    let sizes = vec![(1024, "1KB"), (10 * 1024, "10KB"), (100 * 1024, "100KB")];

    for (size, desc) in sizes {
        let spool = MemoryBackingStore::new();
        let mut message = create_test_message(size);
        let id =
            runtime.block_on(async { spool.write(&mut message).await.expect("Write succeeds") });

        group.throughput(Throughput::Bytes(size as u64));
        group.bench_with_input(BenchmarkId::from_parameter(desc), &size, |b, &_size| {
            b.to_async(&runtime).iter_batched(
                || {
                    // Setup: create spool and write message
                    (spool.clone(), id.clone())
                },
                |(spool, id)| async move {
                    let message = spool.read(black_box(&id)).await.expect("Read succeeds");
                    black_box(message)
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }

    group.finish();
}

fn bench_spool_list(c: &mut Criterion) {
    let mut group = c.benchmark_group("spool_list");

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    let message_counts = vec![10, 100, 1000];

    for count in message_counts {
        let spool = MemoryBackingStore::new();
        runtime.block_on(async {
            for _ in 0..count {
                let mut message = create_test_message(1024);
                spool.write(&mut message).await.expect("Write succeeds");
            }
        });

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{count}_messages")),
            &count,
            |b, &_count| {
                b.to_async(&runtime).iter_batched(
                    || {
                        // Setup: create spool and write multiple messages

                        spool.clone()
                    },
                    |spool| async move {
                        let ids = spool.list().await.expect("List succeeds");
                        black_box(ids)
                    },
                    criterion::BatchSize::SmallInput,
                );
            },
        );
    }

    group.finish();
}

fn bench_spool_full_lifecycle(c: &mut Criterion) {
    let mut group = c.benchmark_group("spool_full_lifecycle");

    let runtime = tokio::runtime::Runtime::new().expect("Failed to create runtime");

    group.bench_function("write_read_delete", |b| {
        b.to_async(&runtime).iter(|| async {
            let spool = MemoryBackingStore::new();
            let mut message = create_test_message(1024);

            // Write
            let id = spool.write(&mut message).await.expect("Write succeeds");

            // Read
            let read_msg = spool.read(&id).await.expect("Read succeeds");
            black_box(read_msg);

            // Delete
            spool.delete(&id).await.expect("Delete succeeds");
        });
    });

    group.finish();
}

// ============================================================================
// Criterion Configuration
// ============================================================================

criterion_group!(
    benches,
    bench_message_creation,
    bench_message_id_operations,
    bench_spool_write,
    bench_spool_read,
    bench_spool_list,
    bench_spool_full_lifecycle,
);
criterion_main!(benches);
