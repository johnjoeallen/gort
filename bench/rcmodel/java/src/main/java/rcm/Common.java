package rcm;

import java.util.concurrent.ConcurrentHashMap;

/** Same workloads as c/rcm.c. Idiomatic Java: GC'd references, ConcurrentHashMap as the shared table. */
public final class Common {
    static final long FNV = 0x100000001b3L, MIX = 0x9E3779B97F4A7C15L;
    static final int NSESS = 1024;
    static int work = 1;

    /** xorshift64*, state in a long[1] (per thread). */
    static long rng(long[] s) {
        long x = s[0];
        x ^= x >>> 12; x ^= x << 25; x ^= x >>> 27;
        s[0] = x;
        return x * 2685821657736338717L;
    }

    // ---- W1/W2
    static final class Entry {
        final long key, val;
        long p0, p1, p2, p3, p4, p5;          // 48-byte payload
        long q0, q1, q2, q3, q4, q5;          // padding, so entries do not share lines
        Entry(long key, long val) {
            this.key = key; this.val = val;
            long b = key & 0xff; p0 = p1 = p2 = p3 = p4 = p5 = b * 0x0101010101010101L;
        }
    }

    static long walk(Entry e, int d) {
        if (d <= 1) return e.val;
        return walk(e, d - 1);
    }

    // ---- W3 types (flattened: headers are a long[24] with name == index, body is long[64], resp body long[32])
    static final class Req { long id; long[] headers; long[] body; }
    static final class Sess {
        final long uid, counter; long d0, d1, d2, d3, d4;
        Sess(long uid, long counter) { this.uid = uid; this.counter = counter; d0 = uid; d1 = uid * 2; d2 = uid * 3; d3 = uid * 4; d4 = uid * 5; }
    }
    static final class Resp { int status = 200; long[] body; }
    static final class Ctx { long acc; int sid; Sess sess; Resp resp; }

    static Req newReq(long[] rs) {
        Req rq = new Req(); rq.id = rng(rs);
        long[] a = new long[24];
        for (int j = 0; j < 8; j++) for (int w = 0; w < 3; w++) a[j * 3 + w] = rng(rs);
        long[] b = new long[64];
        for (int i = 0; i < 64; i++) b[i] = rng(rs);
        rq.headers = a; rq.body = b;
        return rq;
    }

    static long layer(int k, Req rq, Ctx cx, ConcurrentHashMap<Integer, Sess> tb) {
        long r;
        switch (k) {
            case 0 -> { long[] h = rq.headers; cx.acc = h[0] ^ h[1]; }
            case 1 -> cx.acc += rq.id;
            case 2 -> { long[] h = rq.headers; for (int j = 0; j < 8; j++) if (j == 3) cx.acc ^= h[j * 3]; }
            case 3 -> cx.acc = cx.acc * MIX + 1;
            case 4 -> { long[] h = rq.headers; cx.sid = (int) Long.remainderUnsigned(h[15] ^ h[16] ^ h[17], NSESS); cx.acc += cx.sid; }
            case 5 -> {
                Sess s = tb.get(cx.sid);
                cx.acc += s.counter;
                if (Long.remainderUnsigned(rq.id >>> 40, 20) == 0) tb.put(cx.sid, new Sess(s.uid, s.counter + 1));
                cx.sess = s;
            }
            case 6 -> cx.acc ^= (rq.id >>> 8) & 0xff;
            case 7 -> { long[] b = rq.body; long a = cx.acc; for (int w = 0; w < work; w++) for (int i = 0; i < 64; i++) a = (a ^ b[i]) * FNV; cx.acc = a; }
            case 8 -> {
                Sess s = cx.sess; Resp rp = new Resp(); long[] b = new long[32];
                for (int i = 0; i < 32; i++) b[i] = cx.acc + (long) i * s.counter + s.uid;
                cx.acc ^= b[31]; rp.body = b; cx.resp = rp;
            }
            case 9 -> { Resp rp = cx.resp; long[] b = rp.body; long a = cx.acc; for (int w = 0; w < work; w++) for (int i = 0; i < 32; i++) a = (a ^ b[i]) * FNV; cx.acc = a + rp.status; }
            default -> { }
        }
        if (k < 9) r = layer(k + 1, rq, cx, tb); else r = cx.acc;
        return r;
    }

    static long oneRequest(long[] rs, ConcurrentHashMap<Integer, Sess> tb) {
        Req rq = newReq(rs);
        Ctx cx = new Ctx();
        return layer(0, rq, cx, tb);
    }

    static ConcurrentHashMap<Integer, Sess> mkTable() {
        var t = new ConcurrentHashMap<Integer, Sess>(2 * NSESS);
        for (int i = 0; i < NSESS; i++) t.put(i, new Sess(i, i * 7L));
        return t;
    }
}
