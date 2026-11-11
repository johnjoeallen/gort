package rcm;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicLongArray;
import java.util.concurrent.atomic.AtomicReferenceArray;
import rcm.Common.Entry;

/**
 * Comparative W2 driver (same protocol as c/w2sync.c with warm_ms, go/w2x, rust bin w2x).
 * impl=chm: ConcurrentHashMap&lt;Integer,Entry&gt; (as Micro.w2). impl=ara: AtomicReferenceArray&lt;Entry&gt; (lock-free slot array).
 * GC is chosen with JVM flags; the variant label comes from -Drcm.variant.
 */
public final class W2X {
    static final int NENT = 10000, HBUF = 16384, STRIDE = 16;   // 16 longs = 128 bytes between progress counters

    interface Tbl { Entry get(int k); void put(int k, Entry e); }

    static long walk(Entry e, int d) { return d <= 1 ? e.val : walk(e, d - 1); }

    static long fnv(byte[] b, int n, long seed) {
        long h = 0xcbf29ce484222325L ^ seed;
        for (int j = 0; j < n; j++) h = (h ^ (b[j] & 0xff)) * 0x100000001b3L;
        return h;
    }

    static volatile boolean stop;
    static volatile long gsink;

    public static void main(String[] a) throws Exception {
        String impl = Main.gets(a, "impl", "chm"), mode = Main.gets(a, "mode", "hot_same");
        int T = (int) Main.geti(a, "threads", 1), wpct = (int) Main.geti(a, "wpct", 5), hash = (int) Main.geti(a, "hash", 0);
        int warm = (int) Main.geti(a, "warm_ms", 500), dur = (int) Main.geti(a, "dur_ms", 1000);
        if (hash < 0 || hash > HBUF) throw new IllegalArgumentException("hash");
        Tbl t;
        if (impl.equals("chm")) {
            ConcurrentHashMap<Integer, Entry> m = new ConcurrentHashMap<>(2 * NENT);
            for (int i = 0; i < NENT; i++) m.put(i, new Entry(i, i * 5L + 1));
            t = new Tbl() { public Entry get(int k) { return m.get(k); } public void put(int k, Entry e) { m.put(k, e); } };
        } else if (impl.equals("ara")) {
            AtomicReferenceArray<Entry> r = new AtomicReferenceArray<>(NENT);
            for (int i = 0; i < NENT; i++) r.set(i, new Entry(i, i * 5L + 1));
            t = new Tbl() { public Entry get(int k) { return r.get(k); } public void put(int k, Entry e) { r.set(k, e); } };
        } else throw new IllegalArgumentException("impl=chm|ara");

        AtomicLongArray prog = new AtomicLongArray(T * STRIDE + STRIDE);
        Thread[] ths = new Thread[T];
        for (int id0 = 0; id0 < T; id0++) {
            final int id = id0;
            ths[id] = new Thread(() -> {
                long[] s = { 0x9E3779B97F4A7C15L ^ ((id + 1) * 0x1234567L) }, s2 = { s[0] ^ 0x5555 };
                byte[] hb = new byte[HBUF];
                for (int j = 0; j < HBUF; j++) hb[j] = (byte) Common.rng(s2);
                int hot = mode.equals("hot_diff") ? (int) ((id * 601L + 7) % NENT) : 17;
                boolean uni = mode.equals("uniform");
                long sink = 0;
                for (long i = 0; ; i++) {
                    if ((i & 63) == 0) { prog.setOpaque((id + 1) * STRIDE, i); if (stop) break; }
                    long r = Common.rng(s);
                    int k = uni ? (int) Long.remainderUnsigned(r >>> 11, NENT) : hot;
                    long r2 = Common.rng(s);
                    if (Long.remainderUnsigned(r2 >>> 33, 100) < wpct) { t.put(k, new Entry(k, r2 & 0xffff)); continue; }
                    long v = walk(t.get(k), 3);
                    sink += v;
                    if (hash > 0) sink += fnv(hb, hash, v);
                }
                synchronized (W2X.class) { gsink += sink; }
            });
            ths[id].start();
        }
        Thread.sleep(warm);
        long c0 = 0; for (int i = 0; i < T; i++) c0 += prog.getOpaque((i + 1) * STRIDE);
        long w0 = System.nanoTime();
        Thread.sleep(dur);
        long c1 = 0; for (int i = 0; i < T; i++) c1 += prog.getOpaque((i + 1) * STRIDE);
        long w1 = System.nanoTime();
        stop = true;
        for (Thread th : ths) th.join();
        double mops = (c1 - c0) / ((w1 - w0) / 1e9) / 1e6;
        String params = "threads=" + T + ";mode=" + mode + ";wpct=" + wpct + ";ppct=0;hash=" + hash;
        String var = System.getProperty("rcm.variant", "g1");
        System.out.println(String.format(java.util.Locale.ROOT, "W2X,java-%s,%s,%s,mops_total,%.6f", var, impl, params, mops));
        System.out.println(String.format(java.util.Locale.ROOT, "W2X,java-%s,%s,%s,rss_mb,%.1f", var, impl, params, Main.vmhwmMB()));
    }
}
