use as_grep::{Span, SpanKind};
use as_plan::rrf;
use criterion::{criterion_group, criterion_main, Criterion};

fn synthetic_lists(n_lists: usize, list_len: usize) -> Vec<Vec<Span>> {
    (0..n_lists)
        .map(|li| {
            (0..list_len)
                .map(|i| {
                    let uri = format!("doc_{}", (i + li * 7) % (list_len * 2));
                    Span {
                        uri,
                        byte_range: 0..1,
                        line_range: [1, 1],
                        kind: SpanKind::Line,
                        score: 0.0,
                        ..Span::default()
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
