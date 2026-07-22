# Benchmarks

Micro-benchmarks use [criterion](https://github.com/bheisler/criterion.rs) and live in each crate's `benches/`. Run them all with `cargo bench`, or one with `cargo bench -p photon --bench instance_pack`. CI compile-checks the benches (`cargo bench --no-run`) so they cannot rot, but does not run them - timings are machine-dependent.

## photon: `instance_pack`

Packs N sprites into GPU instance bytes: the per-frame CPU work the renderer does before uploading (an affine transform per sprite, then a copy). This is the render loop's measured hotspot - see the [renderer profiling notes](renderer.md). Baseline (release build, one dev machine; treat the numbers as relative, not absolute):

| Sprites | Time | Throughput |
|--------:|-----:|-----------:|
| 1,000 | ~7.4 us | ~135 Melem/s |
| 10,000 | ~82 us | ~123 Melem/s |
| 100,000 | ~5.6 ms | ~18 Melem/s |

Throughput falls off past ~10k as the output (56 bytes per sprite) outgrows cache and the pack becomes memory-bound. The standing optimization is a persistent instance buffer, written in place rather than rebuilt and re-uploaded every frame.
