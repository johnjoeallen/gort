# Comparative W2 hot path: Mops/s total, median (min-max)

### hot_same, hash=0 (no work)

| | T=1 | T=4 | T=8 | T=16 |
|---|---|---|---|---|
| C spin lock, counted (Gort today) | 30.2 (30.2-30.2) | 10.4 (10.4-10.7) | 6.73 (6.63-6.81) | 3.01 (2.93-3.09) |
| C lock-free, counted (QSBR) | 32.0 (32.0-32.0) | 14.6 (14.6-14.6) | 14.1 (14.0-14.1) | 12.3 (12.2-12.4) |
| C borrow, QSBR | 154 (153-154) | 220 (218-220) | 305 (302-305) | 202 (196-205) |
| C borrow, hazard ptr (L4') | 147 (146-148) | 216 (215-216) | 299 (298-300) | 219 (218-221) |
| C borrow, hazard ptr, 100% promoted | 88.5 (88.1-88.6) | 48.5 (48.5-48.8) | 40.5 (40.5-40.6) | 30.1 (30.1-30.2) |
| Go RWMutex stripes | 84.1 (83.8-84.3) | 69.3 (69.0-69.9) | 63.2 (62.1-63.4) | 59.2 (56.3-61.0) |
| Go atomic.Pointer | 167 (164-168) | 309 (307-310) | 340 (339-341) | 298 (266-302) |
| Java G1 ConcurrentHashMap | 235 (234-251) | 165 (163-177) | 149 (148-156) | 107 (102-135) |
| Java ZGC ConcurrentHashMap | 196 (195-197) | 188 (176-190) | 150 (148-153) | 101 (98-126) |
| Java G1 AtomicReferenceArray | 305 (303-306) | 318 (261-363) | 325 (295-350) | 193 (169-271) |
| Java ZGC AtomicReferenceArray | 255 (255-257) | 340 (338-361) | 370 (369-413) | 279 (279-281) |
| Rust RwLock<Arc> stripes | 60.0 (60.0-60.0) | 19.5 (19.3-19.6) | 12.5 (12.4-12.6) | 7.44 (7.39-7.51) |
| Rust arc-swap | 82.4 (82.2-82.5) | 99.2 (97.2-99.5) | 100.0 (98.0-100.4) | 59.8 (59.6-60.2) |
| Rust crossbeam-epoch | 115 (114-115) | 177 (177-179) | 236 (235-236) | 155 (151-157) |

### hot_same, hash=1500 (~1 us real work per read)

| | T=1 | T=4 | T=8 | T=16 |
|---|---|---|---|---|
| C spin lock, counted (Gort today) | 0.97 (0.97-0.97) | 3.80 (3.73-3.81) | 7.30 (7.25-7.35) | 4.01 (3.75-4.15) |
| C lock-free, counted (QSBR) | 0.97 (0.97-0.97) | 3.78 (3.78-3.78) | 7.29 (7.29-7.30) | 11.6 (11.6-11.6) |
| C borrow, QSBR | 0.98 (0.98-0.98) | 3.88 (3.88-3.88) | 7.67 (7.66-7.67) | 14.6 (14.6-14.6) |
| C borrow, hazard ptr (L4') | 0.98 (0.98-0.98) | 3.88 (3.88-3.88) | 7.66 (7.66-7.67) | 14.5 (14.5-14.5) |
| C borrow, hazard ptr, 100% promoted | 0.98 (0.97-0.98) | 3.83 (3.82-3.83) | 7.53 (7.53-7.54) | 13.6 (13.6-13.6) |
| Go RWMutex stripes | 0.97 (0.97-0.97) | 3.57 (3.57-3.59) | 3.93 (3.81-3.98) | 2.54 (1.75-3.13) |
| Go atomic.Pointer | 0.98 (0.97-0.98) | 3.88 (3.88-3.88) | 7.62 (7.55-7.63) | 14.2 (13.8-14.3) |
| Java G1 ConcurrentHashMap | 0.98 (0.98-0.98) | 3.89 (3.88-3.89) | 7.68 (7.67-7.68) | 14.6 (14.5-14.6) |
| Java ZGC ConcurrentHashMap | 0.98 (0.98-0.98) | 3.89 (3.89-3.89) | 7.67 (7.67-7.67) | 14.6 (14.6-14.7) |
| Java G1 AtomicReferenceArray | 0.98 (0.98-0.98) | 3.90 (3.90-3.90) | 7.70 (7.70-7.70) | 14.6 (14.5-14.7) |
| Java ZGC AtomicReferenceArray | 0.98 (0.98-0.98) | 3.90 (3.89-3.90) | 7.70 (7.69-7.70) | 14.7 (14.7-14.7) |
| Rust RwLock<Arc> stripes | 0.98 (0.98-0.98) | 3.77 (3.76-3.78) | 7.37 (7.36-7.37) | 9.50 (9.38-9.60) |
| Rust arc-swap | 0.98 (0.97-0.98) | 3.87 (3.87-3.88) | 7.63 (7.62-7.63) | 13.8 (13.8-13.8) |
| Rust crossbeam-epoch | 0.98 (0.97-0.98) | 3.87 (3.87-3.88) | 7.67 (7.66-7.67) | 14.6 (14.6-14.6) |

### hot_same, hash=15000 (~10 us real work per read)

| | T=1 | T=4 | T=8 | T=16 |
|---|---|---|---|---|
| C spin lock, counted (Gort today) | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.52 (1.52-1.52) |
| C lock-free, counted (QSBR) | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.52 (1.51-1.52) |
| C borrow, QSBR | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.53 (1.53-1.53) |
| C borrow, hazard ptr (L4') | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.53 (1.52-1.53) |
| C borrow, hazard ptr, 100% promoted | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.52 (1.51-1.52) |
| Go RWMutex stripes | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.76 (0.76-0.76) | 0.69 (0.68-0.70) |
| Go atomic.Pointer | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.76-0.77) | 1.52 (1.39-1.53) |
| Java G1 ConcurrentHashMap | 0.10 (0.10-0.10) | 0.39 (0.38-0.39) | 0.77 (0.77-0.77) | 1.53 (0.28-1.53) |
| Java ZGC ConcurrentHashMap | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.48 (0.27-1.53) |
| Java G1 AtomicReferenceArray | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.53 (1.16-1.53) |
| Java ZGC AtomicReferenceArray | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.53 (1.53-1.53) |
| Rust RwLock<Arc> stripes | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.52 (1.50-1.52) |
| Rust arc-swap | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.53 (1.53-1.53) |
| Rust crossbeam-epoch | 0.10 (0.10-0.10) | 0.39 (0.39-0.39) | 0.77 (0.77-0.77) | 1.54 (1.54-1.54) |

### hot_same, hash=1500: scaling efficiency vs own single thread (100% = perfect)

| | T=4 | T=8 | T=16 |
|---|---|---|---|
| C spin lock, counted (Gort today) | 98% | 94% | 26% |
| C lock-free, counted (QSBR) | 97% | 94% | 75% |
| C borrow, QSBR | 99% | 98% | 94% |
| C borrow, hazard ptr (L4') | 99% | 98% | 93% |
| C borrow, hazard ptr, 100% promoted | 98% | 96% | 87% |
| Go RWMutex stripes | 92% | 50% | 16% |
| Go atomic.Pointer | 99% | 98% | 91% |
| Java G1 ConcurrentHashMap | 99% | 98% | 93% |
| Java ZGC ConcurrentHashMap | 99% | 98% | 94% |
| Java G1 AtomicReferenceArray | 99% | 98% | 93% |
| Java ZGC AtomicReferenceArray | 99% | 98% | 94% |
| Rust RwLock<Arc> stripes | 96% | 94% | 61% |
| Rust arc-swap | 99% | 98% | 88% |
| Rust crossbeam-epoch | 99% | 98% | 94% |

### hot_same, hash=15000: scaling efficiency vs own single thread (100% = perfect)

| | T=4 | T=8 | T=16 |
|---|---|---|---|
| C spin lock, counted (Gort today) | 100% | 99% | 98% |
| C lock-free, counted (QSBR) | 100% | 99% | 98% |
| C borrow, QSBR | 100% | 99% | 99% |
| C borrow, hazard ptr (L4') | 100% | 99% | 99% |
| C borrow, hazard ptr, 100% promoted | 100% | 99% | 98% |
| Go RWMutex stripes | 100% | 98% | 44% |
| Go atomic.Pointer | 100% | 99% | 98% |
| Java G1 ConcurrentHashMap | 101% | 100% | 100% |
| Java ZGC ConcurrentHashMap | 101% | 100% | 97% |
| Java G1 AtomicReferenceArray | 101% | 100% | 100% |
| Java ZGC AtomicReferenceArray | 101% | 100% | 100% |
| Rust RwLock<Arc> stripes | 100% | 99% | 98% |
| Rust arc-swap | 100% | 99% | 99% |
| Rust crossbeam-epoch | 100% | 99% | 99% |

### hot_diff reference, T=16, no work

| | Mops/s |
|---|---|
| C spin lock, counted (Gort today) | 479 (479-479) |
| C lock-free, counted (QSBR) | 476 (475-476) |
| C borrow, QSBR | 1285 (1282-1287) |
| C borrow, hazard ptr (L4') | 2063 (2060-2067) |
| C borrow, hazard ptr, 100% promoted | 1298 (1297-1300) |
| Go RWMutex stripes | 560 (512-597) |
| Go atomic.Pointer | 785 (774-839) |
| Java G1 ConcurrentHashMap | 1041 (842-1509) |
| Java ZGC ConcurrentHashMap | 564 (487-675) |
| Java G1 AtomicReferenceArray | 1818 (1534-2081) |
| Java ZGC AtomicReferenceArray | 1779 (1152-2587) |
| Rust RwLock<Arc> stripes | 945 (944-945) |
| Rust arc-swap | 84.9 (84.0-85.2) |
| Rust crossbeam-epoch | 505 (492-532) |

### peak RSS MB, hot_same T=16

| | hash=0 | hash=15000 |
|---|---|---|
| C spin lock, counted (Gort today) | 4.10 (3.88-4.14) | 3.88 (3.83-3.91) |
| C lock-free, counted (QSBR) | 4.05 (3.84-4.14) | 3.88 (3.88-4.11) |
| C borrow, QSBR | 47.0 (36.7-53.1) | 3.91 (3.88-4.12) |
| C borrow, hazard ptr (L4') | 8.36 (8.08-8.39) | 7.88 (7.30-8.00) |
| C borrow, hazard ptr, 100% promoted | 7.83 (7.64-7.90) | 7.90 (7.83-8.11) |
| Go RWMutex stripes | 9.40 (9.30-10.90) | 9.00 (8.70-10.10) |
| Go atomic.Pointer | 43.7 (31.1-58.2) | 8.90 (8.60-9.00) |
| Java G1 ConcurrentHashMap | 949 (948-961) | 99.8 (95.9-100.8) |
| Java ZGC ConcurrentHashMap | 1028 (964-1208) | 74.5 (64.0-76.8) |
| Java G1 AtomicReferenceArray | 976 (976-1222) | 98.4 (96.7-99.4) |
| Java ZGC AtomicReferenceArray | 2554 (2541-2561) | 74.3 (74.1-75.1) |
| Rust RwLock<Arc> stripes | 5.50 (5.40-5.60) | 5.50 (5.40-5.50) |
| Rust arc-swap | 5.40 (5.30-5.40) | 5.30 (5.20-5.40) |
| Rust crossbeam-epoch | 30.9 (27.6-32.4) | 5.10 (5.10-5.30) |
