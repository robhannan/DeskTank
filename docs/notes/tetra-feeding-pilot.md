# Repeated feeding of neon tetras (#434, part of #432)

How the neon tetra pilot was measured and tuned, so the numbers can be
reproduced and the trade-offs weighed again.

## Recipe

`crates/desktank-sim/examples/tetra_feeding.rs` uses only APIs that exist at
the pre-needs commit `81a44bd`, so the same file builds there (copy it into a
checkout of that commit) and at the #439 commit `9d7b029`:

```sh
cargo run --release -p desktank-sim --example tetra_feeding            # 12 schools, markdown table
cargo run --release -p desktank-sim --example tetra_feeding -- --seeds 48
cargo run --release -p desktank-sim --example tetra_feeding -- --csv   # one row per flake
cargo run --release -p desktank-sim --example tetra_feeding -- --trace 6 > tuned-6.csv
```

Each seeded school of six runs continuously after 12 s of settling, so every
fish carries its feeding history forward. One flake drops at the surface on
alternating sides (cells 54 and 106 of 160), 12 times, at 15 s and 90 s
intervals, plus a matching 12 × 15 s run with no food. A fish "approaches"
when it ends up 10 cells nearer the drop within 2 s. The replay in
`scripts/tetra-feeding-replay.ts` draws `--trace` files from several builds
side by side with Inspect closed (`desktank-sim` regression tests live in
`crates/desktank-sim/tests/tetra_feeding.rs`).

## Results (12 seeded schools, 144 flakes per row)

| build | interval | approach in 2 s (of 6) | eaten in 10 s | first bite | repeat first eater | busiest fish share | approach after a meal / otherwise | school ignores a flake |
|---|---|---|---|---|---|---|---|---|
| pre-needs `81a44bd` | 15 s | 5.85 | 142/144 | 4.3 s | 24% | 42% | 98% / 97% | 1 |
| #439 `9d7b029` | 15 s | 3.69 | 144/144 | 4.6 s | 21% | 35% | 67% / 59% | 6 |
| tuned | 15 s | 3.38 | 141/144 | 4.8 s | 16% | 34% | 37% / 59% | 9 |
| pre-needs | 90 s | 5.94 | 144/144 | 4.4 s | 29% | 46% | 99% / 99% | 0 |
| #439 | 90 s | 4.08 | 144/144 | 4.8 s | 28% | 46% | 71% / 67% | 2 |
| tuned | 90 s | 3.88 | 143/144 | 5.0 s | 25% | 43% | 73% / 63% | 3 |
| all three | no food | 0.26 | – | – | – | – | – | 121 of 144 quiet |

With no food, turns per fish per minute (10.9) and the gap to the nearest
mate (7.9 cells) are identical in all three builds. At 48 schools the picture
holds: the share of first bites taken by a fish that bit one of the previous
two flakes is 35% (31% expected by chance) tuned against 42% (29%) at #439,
and repeat first eaters fall from 25% to 14% at 15 s.

## What the data showed

- #439 already cuts the rush (5.9 to 3.7 fish) but a fish that had just eaten
  was as likely to go for the next flake as any other (67% against 59%), and
  recent eaters won more than their share of first bites. One flake gives only
  a quarter of a full belly, and linear satiety left every fish in a school
  fed one flake per 15 s looking starving.
- At long intervals the bite has worn off, every fish is starving again, and
  the fastest fish wins. The busiest fish took 41–46% of first bites in all
  builds against about 31% by chance; the cause is the per-fish speed quirk
  (85–115%), which this pilot does not change.

## The tuning

- **Appetite** (`needs.rs`): the first bite's worth of satiety (25%) counts
  three times as much towards feeling fed, the other 75% a third as much.
  A fish that has just eaten drops to a quarter of its appetite and recovers
  it over the half-minute that bite lasts. It reads only stored satiety, so
  save v20 and away-time behaviour are unchanged. The Inspect Hunger reading
  still shows plain satiety.
- **Company** (`needs.rs`): a school's pull for a fish already in it rises
  from 16 to 50, so a fish that is not hungry stays with it rather than
  leaving for a flake some way off.
- Stronger settings (a fourfold first bite, company 60) cut repeat first
  eaters to 1% but made a just-fed fish hang back almost always, and cost
  first-bite time. Weaker ones left #439's behaviour. A slowly drifting hunger
  mood changed nothing at long intervals and was dropped.

## Trade-offs and what shows

- Visible: after a flake, a fish that ate in the last half-minute often trails
  the school while the hungry ones leave (replay, flake 2, fish 2); after a
  minute or so everyone goes again. Three of 144 flakes at 15 s were not eaten
  within 10 s, as with the pre-needs build (2 of 144); a flake that stays is
  taken by whoever is hungry.
- The school is slower to react by about half a second, and slightly more often
  nobody moves at all within 2 s (9 flakes in 144, none left uneaten).
- Not fixed: the same fast fish winning at long intervals. Pace is a
  stable individual trait, and tuning needs cannot remove it without making
  every fish the same.
- Idle cost is unchanged: no new state or per-tick work beyond the existing
  decision every second.
