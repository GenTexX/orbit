//! Benchmark the per-frame CPU hotspot: packing sprites into GPU instance bytes.

use std::hint::black_box;

use criterion::{BenchmarkId, Criterion, Throughput, criterion_group, criterion_main};
use glam::Vec2;
use photon::{Color, Sprite, pack_sprite_instances};

/// A representative field of sprites: varied positions, rotations, and tints, so
/// the affine math (sin/cos per sprite) is actually exercised.
fn make_sprites(n: usize) -> Vec<Sprite> {
    (0..n)
        .map(|i| {
            let f = i as f32;
            let mut sprite = Sprite::new(Vec2::new(f * 1.3, f * 0.7), Vec2::new(8.0, 8.0));
            sprite.rotation = f * 0.01;
            sprite.tint = Color::hsv(f * 0.001, 0.7, 1.0);
            sprite
        })
        .collect()
}

fn bench_instance_pack(c: &mut Criterion) {
    let mut group = c.benchmark_group("instance_pack");
    for n in [1_000usize, 10_000, 100_000] {
        let sprites = make_sprites(n);
        group.throughput(Throughput::Elements(n as u64));
        group.bench_with_input(BenchmarkId::from_parameter(n), &sprites, |b, sprites| {
            b.iter(|| pack_sprite_instances(black_box(sprites)));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_instance_pack);
criterion_main!(benches);
