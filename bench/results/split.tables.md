### Allocation cost, ns per object (median, range of 5 runs)

| body | operation | combined | split | split vs combined |
|---|---|---|---|---|
| 16 B | allocate, sequential | 6.39 (6.06 to 6.54) | 9.29 (9.09 to 9.79) | +45% |
| 16 B | free, random order | 10.42 (9.83 to 11.11) | 24.47 (21.85 to 25.11) | +135% |
| 16 B | free + allocate (churn) | 13.52 (13.34 to 16.91) | 40.17 (37.98 to 44.22) | +197% |
| 64 B | allocate, sequential | 12.96 (12.69 to 13.31) | 16.33 (16.13 to 16.56) | +26% |
| 64 B | free, random order | 16.79 (16.53 to 16.90) | 29.65 (29.08 to 31.78) | +77% |
| 64 B | free + allocate (churn) | 26.26 (25.88 to 27.78) | 55.32 (54.76 to 57.48) | +111% |
| 256 B | allocate, sequential | 41.23 (41.03 to 42.99) | 45.05 (44.46 to 45.69) | +9% |
| 256 B | free, random order | 27.77 (27.31 to 28.46) | 43.94 (40.88 to 44.06) | +58% |
| 256 B | free + allocate (churn) | 30.75 (30.07 to 31.66) | 63.27 (60.61 to 65.39) | +106% |

### Dereference cost, ns per access, 64 B body (median, range of 5 runs)

| working set (combined) | pattern | combined | split | split vs combined |
|---|---|---|---|---|
| about 160 KB | sequential | 0.329 (0.314 to 0.355) | 0.443 (0.417 to 0.446) | +35% |
| about 160 KB | random, independent | 0.301 (0.295 to 0.312) | 0.525 (0.524 to 0.526) | +74% |
| about 160 KB | pointer chase | 2.553 (2.549 to 2.554) | 4.468 (4.468 to 4.472) | +75% |
| about 8 MB | sequential | 0.513 (0.508 to 0.514) | 0.643 (0.640 to 0.650) | +25% |
| about 8 MB | random, independent | 0.736 (0.670 to 0.742) | 1.267 (1.254 to 1.271) | +72% |
| about 8 MB | pointer chase | 10.557 (10.538 to 10.561) | 21.843 (21.835 to 21.883) | +107% |
| about 120 MB | sequential | 1.143 (1.121 to 1.199) | 1.515 (1.457 to 1.593) | +33% |
| about 120 MB | random, independent | 2.689 (2.675 to 2.892) | 4.982 (4.897 to 5.206) | +85% |
| about 120 MB | pointer chase | 35.695 (33.693 to 47.950) | 97.848 (88.056 to 99.371) | +174% |
| about 480 MB | sequential | 1.618 (1.586 to 1.846) | 1.833 (1.785 to 1.846) | +13% |
| about 480 MB | random, independent | 3.939 (3.925 to 3.982) | 7.765 (7.738 to 7.811) | +97% |
| about 480 MB | pointer chase | 88.616 (88.470 to 88.969) | 168.179 (165.227 to 171.287) | +90% |

### Retained memory after a burst, MB resident above the baseline (median of 3)

| scenario | layout | after allocating | after freeing the rest | after releasing free pages | after moving bodies | move pass, ms | body chunks freed |
|---|---|---|---|---|---|---|---|
| 64 B, 1.0% kept, scatter | combined | 157 | 157 | 70 | n/a | n/a | n/a |
| 64 B, 1.0% kept, scatter | split | 192 | 192 | 110 | 44 | 7 | 2247 |
| 64 B, 10.0% kept, scatter | combined | 157 | 157 | 157 | n/a | n/a | n/a |
| 64 B, 10.0% kept, scatter | split | 192 | 192 | 192 | 64 | 15 | 2044 |
| 64 B, 1.0% kept, cluster | combined | 157 | 2 | 2 | n/a | n/a | n/a |
| 64 B, 1.0% kept, cluster | split | 192 | 2 | 2 | 2 | 0 | 0 |
| 256 B, 1.0% kept, scatter | combined | 263 | 237 | 51 | n/a | n/a | n/a |
| 256 B, 1.0% kept, scatter | split | 280 | 258 | 72 | 24 | 7 | 3681 |

### Every object dead, every one still held by a WeakRef: resident MB (median of 3)

| objects | layout | all live | all dead, weak refs held | weak refs dropped |
|---|---|---|---|---|
| 64 B x 2000000 | combined | 157 | 157 | 0.1 |
| 64 B x 2000000 | split | 192 | 50 | 0.2 |
| 256 B x 1000000 | combined | 263 | 263 | 0.1 |
| 256 B x 1000000 | split | 280 | 25 | 0.2 |

