use as_core::Hit;
use as_plan::rrf;
use criterion::{criterion_group, criterion_main, Criterion};

fn synthetic_lists(n_lists: usize, list_len: usize) -> Vec<Vec<Hit>> {
    (0..n_lists)
        .map(|li| {
            (0..list_len)
                .map(|i| {
                    let id = format!("doc_{}", (i + li * 7) % (list_len * 2));
                    Hit {
                        id: id.clone(),
                        uri: id,
                        score: 0.0,
                        snippet: None,
                        metadata: serde_json::Value::Null,
                    }
                })
                .collect()
        })
        .collect()
}

fn bench_rrf(c: &mut Criterion) {
    let lists = synthetic_lists(3, 1000);
    c.bench_function("rrf_3x1000_top10", |b| {
        b.iter(|| rrf(&lists, 60, 10));
    });
}

criterion_group!(benches, bench_rrf);
criterion_main!(benches);
