import sys, re, statistics as st
from collections import defaultdict
rows = defaultdict(list)
for line in open(sys.argv[1]):
    if not line.startswith("RESULT"): continue
    kv = dict(p.split("=", 1) for p in line.split()[1:])
    key = (kv["work"], kv["mode"], kv["n"], kv["body"], kv.get("keep_bp", ""), kv.get("pattern", ""))
    rows[key].append(kv)
def vals(work, mode, n, body, field, keep="", pat=""):
    return [float(r[field]) for r in rows.get((work, mode, n, body, keep, pat), [])]
def cell(v, fmt="%.2f"):
    if not v: return "-"
    return (fmt + " (%s to %s)") % (st.median(v), fmt % min(v), fmt % max(v)) if len(v) > 1 and min(v) != max(v) else fmt % st.median(v)
def med(v): return st.median(v) if v else float("nan")
def ratio(a, b): return "%+.0f%%" % ((med(b) / med(a) - 1) * 100) if a and b and med(a) else "-"
def table(title, head, lines):
    print("### " + title + "\n"); print("| " + " | ".join(head) + " |"); print("|" + "---|" * len(head))
    for l in lines: print("| " + " | ".join(l) + " |")
    print()
# alloc
L = []
for body in ("16", "64", "256"):
    for f, name in (("fill_ns", "allocate, sequential"), ("free_random_ns", "free, random order"), ("churn_pair_ns", "free + allocate (churn)")):
        a, b = vals("alloc", "combined", "1500000", body, f), vals("alloc", "split", "1500000", body, f)
        L.append([body + " B", name, cell(a), cell(b), ratio(a, b)])
table("Allocation cost, ns per object (median, range of 5 runs)", ["body", "operation", "combined", "split", "split vs combined"], L)
# deref
L = []
for n, ws in (("2000", "about 160 KB"), ("100000", "about 8 MB"), ("1500000", "about 120 MB"), ("6000000", "about 480 MB")):
    for f, name in (("seq_ns", "sequential"), ("random_ns", "random, independent"), ("chase_ns", "pointer chase")):
        a, b = vals("deref", "combined", n, "64", f), vals("deref", "split", n, "64", f)
        L.append([ws, name, cell(a, "%.3f"), cell(b, "%.3f"), ratio(a, b)])
table("Dereference cost, ns per access, 64 B body (median, range of 5 runs)", ["working set (combined)", "pattern", "combined", "split", "split vs combined"], L)
# retain
L = []
for n, body, keep, pat in (("2000000", "64", "100", "scatter"), ("2000000", "64", "1000", "scatter"), ("2000000", "64", "100", "cluster"), ("1000000", "256", "100", "scatter")):
    for mode in ("combined", "split"):
        g = lambda f: med(vals("retain", mode, n, body, f, keep, pat))
        L.append(["%s B, %s%% kept, %s" % (body, int(keep) / 100, pat), mode, "%.0f" % (g("rss_full_kb") / 1024), "%.0f" % (g("rss_after_free_kb") / 1024),
                  "%.0f" % (g("rss_after_pagerelease_kb") / 1024), "%.0f" % (g("rss_after_evac_kb") / 1024) if mode == "split" else "n/a",
                  "%.0f" % g("evac_ms") if mode == "split" else "n/a", "%.0f" % g("chunks_freed") if mode == "split" else "n/a"])
table("Retained memory after a burst, MB resident above the baseline (median of 3)",
      ["scenario", "layout", "after allocating", "after freeing the rest", "after releasing free pages", "after moving bodies", "move pass, ms", "body chunks freed"], L)
# weak
L = []
for n, body in (("2000000", "64"), ("1000000", "256")):
    for mode in ("combined", "split"):
        g = lambda f: med(vals("weak", mode, n, body, f))
        L.append(["%s B x %s" % (body, n), mode, "%.0f" % (g("rss_full_kb") / 1024), "%.0f" % (g("rss_dead_with_weak_kb") / 1024), "%.1f" % (g("rss_after_weak_dropped_kb") / 1024)])
table("Every object dead, every one still held by a WeakRef: resident MB (median of 3)", ["objects", "layout", "all live", "all dead, weak refs held", "weak refs dropped"], L)
