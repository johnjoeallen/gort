Median picoseconds per iteration, gortc output, `-O2`, one thread pinned to one core.

| workload | ref | shared | ref built with --atomic-refs | shared / ref |
|---|---|---|---|---|
| store | 573 | 7241 | 7248 | 12.6x |
| sync | 543 | 7783 | 8507 | 14.3x |
| alloc | 10320 | 13933 | 13936 | 1.4x |
| list | 14652 | 68227 | 37839 | 4.7x |
