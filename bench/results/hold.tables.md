## Steady state: 50% hits, 200 us database, 4 workers, 16 locks, a 500 us hold after every hit

Identical responses in all four configurations: yes.

| Configuration | Work per request (ns) | Lock retries per request | Requests/s | p50 (us) | p99 (us) | Resident at end (MB) |
|---|---|---|---|---|---|---|
| combined, copy held | 501 | 0.01 | 2,929,851 | 240 | 501 | 54 |
| combined, Ref held | 502 | 0.01 | 2,931,912 | 240 | 501 | 54 |
| split, copy held | 527 | 0.02 | 2,928,377 | 240 | 501 | 63 |
| split, Ref held | 523 | 0.02 | 2,930,211 | 240 | 501 | 63 |

## After a flush, by hold time (50% hits, 200 us database, 4 workers, 16 locks, median of 3)

| Hold (us) | Configuration | Resident at end (MB) | Mapped (MB) | Work per request (ns) | Requests/s | Mover attempts | Chunks freed | Skipped (pinned) | Skip share | Bodies moved | Longest hold (us) |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 100 | combined, Ref held | 54 | 39 | 445 | 7,986,329 | - | - | - | - | - | - |
| 100 | split, copy held, no mover | 63 | 48 | 463 | 7,919,021 | - | - | - | - | - | - |
| 100 | split, copy held, mover | 46 | 30 | 472 | 7,758,032 | 309 | 309 | 0 | 0% | 37,211 | 71 |
| 100 | split, Ref held, no mover | 63 | 48 | 462 | 7,917,419 | - | - | - | - | - | - |
| 100 | split, Ref held, mover | 47 | 32 | 469 | 7,778,644 | 326 | 294 | 32 | 10% | 32,407 | 59 |
| 500 | combined, Ref held | 54 | 39 | 466 | 2,372,620 | - | - | - | - | - | - |
| 500 | split, copy held, no mover | 63 | 48 | 483 | 2,399,964 | - | - | - | - | - | - |
| 500 | split, copy held, mover | 35 | 20 | 484 | 2,283,681 | 1,105 | 1,104 | 1 | 0% | 156,239 | 31 |
| 500 | split, Ref held, no mover | 63 | 48 | 482 | 2,399,976 | - | - | - | - | - | - |
| 500 | split, Ref held, mover | 35 | 20 | 479 | 2,277,145 | 1,426 | 749 | 683 | 48% | 98,773 | 57 |
| 2000 | combined, Ref held | 54 | 39 | 537 | 642,086 | - | - | - | - | - | - |
| 2000 | split, copy held, no mover | 63 | 48 | 552 | 644,305 | - | - | - | - | - | - |
| 2000 | split, copy held, mover | 35 | 20 | 578 | 644,161 | 2,417 | 2,414 | 2 | 0% | 608,319 | 78 |
| 2000 | split, Ref held, no mover | 63 | 48 | 546 | 643,111 | - | - | - | - | - | - |
| 2000 | split, Ref held, mover | 35 | 20 | 582 | 642,400 | 3,156 | 693 | 2,437 | 77% | 69,065 | 60 |
| 10000 | combined, Ref held | 54 | 39 | 635 | 133,697 | - | - | - | - | - | - |
| 10000 | split, copy held, no mover | 63 | 48 | 620 | 133,886 | - | - | - | - | - | - |
| 10000 | split, copy held, mover | 35 | 20 | 692 | 133,603 | 3,645 | 3,644 | 1 | 0% | 784,029 | 83 |
| 10000 | split, Ref held, no mover | 63 | 48 | 610 | 133,734 | - | - | - | - | - | - |
| 10000 | split, Ref held, mover | 35 | 20 | 655 | 133,667 | 6,608 | 1,465 | 5,143 | 78% | 116,067 | 53 |
