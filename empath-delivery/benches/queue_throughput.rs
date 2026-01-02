use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use empath_delivery::queue::DeliveryQueue;
use empath_spool::SpooledMessageId;

fn bench_queue_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("delivery_queue_throughput");
    let sizes = [100usize, 1_000, 10_000];

    for size in sizes {
        group.throughput(Throughput::Elements(size as u64));
        group.bench_with_input(BenchmarkId::new("enqueue", size), &size, |b, &size| {
            b.iter(|| {
                let queue = DeliveryQueue::new();
                for _ in 0..size {
                    let message_id = SpooledMessageId::generate();
                    queue.enqueue(message_id, "example.com".to_string());
                }
                black_box(queue.len())
            });
        });

        group.bench_with_input(
            BenchmarkId::new("enqueue_remove", size),
            &size,
            |b, &size| {
                b.iter(|| {
                    let queue = DeliveryQueue::new();
                    let mut ids = Vec::with_capacity(size);
                    for _ in 0..size {
                        let message_id = SpooledMessageId::generate();
                        queue.enqueue(message_id.clone(), "example.com".to_string());
                        ids.push(message_id);
                    }
                    for message_id in ids {
                        queue.remove(&message_id);
                    }
                    black_box(queue.len())
                });
            },
        );

        group.bench_with_input(
            BenchmarkId::new("pending_iteration", size),
            &size,
            |b, &size| {
                let queue = DeliveryQueue::new();
                for _ in 0..size {
                    let message_id = SpooledMessageId::generate();
                    queue.enqueue(message_id, "example.com".to_string());
                }

                b.iter(|| {
                    let mut count = 0usize;
                    for _ in queue.pending_messages_iter() {
                        count += 1;
                    }
                    black_box(count)
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_queue_throughput);
criterion_main!(benches);
