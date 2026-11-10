package rcm;

import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;
import org.openjdk.jmh.annotations.*;
import rcm.Common.Entry;

/**
 * JMH micro-benchmarks.
 * W1: one op = one chain of `depth` calls (ns/pass = score / depth).
 * W2: Throughput, ops/us summed over all threads (= total Mops/s). Thread count comes from the JMH -t flag.
 */
public class Micro {
    @State(Scope.Thread)
    public static class W1S {
        @Param({"1", "4", "16"}) public int depth;
        final Entry[] pool = new Entry[64];
        int i;
        @Setup public void setup() { for (int k = 0; k < 64; k++) pool[k] = new Entry(k, k * 3 + 1); }
    }

    @Benchmark @BenchmarkMode(Mode.AverageTime) @OutputTimeUnit(TimeUnit.NANOSECONDS)
    public long w1(W1S s) { return Common.walk(s.pool[s.i++ & 63], s.depth); }

    @State(Scope.Benchmark)
    public static class W2Shared {
        @Param({"uniform", "hot_same", "hot_diff"}) public String mode;
        static final int NENT = 10000;
        ConcurrentHashMap<Integer, Entry> t;
        final AtomicInteger ids = new AtomicInteger();
        @Setup(Level.Trial) public void setup() {
            t = new ConcurrentHashMap<>(2 * NENT);
            for (int i = 0; i < NENT; i++) t.put(i, new Entry(i, i * 5L + 1));
            ids.set(0);
        }
    }
    @State(Scope.Thread)
    public static class W2T {
        long[] s = new long[1]; int hot; boolean uni;
        @Setup(Level.Trial) public void setup(W2Shared sh) {
            int id = sh.ids.getAndIncrement();
            s[0] = 0x9E3779B97F4A7C15L ^ ((id + 1) * 0x1234567L);
            uni = sh.mode.equals("uniform");
            hot = sh.mode.equals("hot_same") ? 17 : (int) ((id * 601L + 7) % W2Shared.NENT);
        }
    }

    @Benchmark @BenchmarkMode(Mode.Throughput) @OutputTimeUnit(TimeUnit.MICROSECONDS)
    public long w2(W2Shared sh, W2T ts) {
        long r = Common.rng(ts.s);
        int k = ts.uni ? (int) (Long.remainderUnsigned(r >>> 11, W2Shared.NENT)) : ts.hot;
        long r2 = Common.rng(ts.s);
        if (Long.remainderUnsigned(r2 >>> 33, 100) >= 5) {
            return Common.walk(sh.t.get(k), 3);
        } else {
            sh.t.put(k, new Entry(k, r2 & 0xffff));
            return 0;
        }
    }
}
