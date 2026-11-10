package rcm;

import java.lang.management.ManagementFactory;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.Arrays;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.CountDownLatch;
import java.util.concurrent.CyclicBarrier;
import java.util.concurrent.Executors;
import rcm.Common.*;

public final class Main {
    static String params = "", variant = System.getProperty("rcm.variant", "Java-g1"), impl = "java";

    static void row(String w, String metric, double v) { System.out.println(String.format(java.util.Locale.ROOT, "%s,%s,%s,%s,%s,%.6f", w, impl, variant, params, metric, v)); }
    static long geti(String[] a, String k, long d) { for (String s : a) if (s.startsWith(k + "=")) return Long.parseLong(s.substring(k.length() + 1)); return d; }
    static String gets(String[] a, String k, String d) { for (String s : a) if (s.startsWith(k + "=")) return s.substring(k.length() + 1); return d; }
    static double vmhwmMB() {
        try { for (String l : Files.readAllLines(Path.of("/proc/self/status"))) if (l.startsWith("VmHWM:")) return Double.parseDouble(l.substring(6).replace("kB", "").trim()) / 1024; } catch (Exception e) { }
        return 0;
    }
    static double cpuS() { return ((com.sun.management.OperatingSystemMXBean) ManagementFactory.getOperatingSystemMXBean()).getProcessCpuTime() / 1e9; }

    static void w3(String[] args) throws Exception {
        final int T = (int) geti(args, "workers", 1); final int total = (int) geti(args, "requests", 5000000);
        final int warm = (int) geti(args, "warmup", 300000); final int prewarm = (int) geti(args, "prewarm", 1500000);
        Common.work = (int) geti(args, "work", 1);
        final String tbm = gets(args, "table", "shared"); final boolean priv = tbm.equals("private");
        final String threads = gets(args, "threads", "virtual");
        params = "workers=" + T + ";table=" + tbm + ";work=" + Common.work;
        final ConcurrentHashMap<Integer, Sess> shared = priv ? null : Common.mkTable();
        final int n = total / T; final int[] lat = new int[n * T]; final long[] cs = new long[T], t0s = new long[T], t1s = new long[T];
        final CyclicBarrier bar = new CyclicBarrier(T);
        final CountDownLatch done = new CountDownLatch(T);
        Runnable[] rs = new Runnable[T];
        for (int id = 0; id < T; id++) {
            final int fid = id;
            rs[id] = () -> {
                try {
                    long[] st = { 0x9E3779B97F4A7C15L ^ ((fid + 1) * 0x1234567L) };
                    var tb = priv ? Common.mkTable() : shared;
                    // JIT pre-warm on a throwaway table/RNG so the shared table's evolution matches the other languages
                    long[] ps = { 0x1234567L + fid }; var pt = Common.mkTable(); long pc = 0;
                    for (int i = 0; i < prewarm; i++) pc += Common.oneRequest(ps, pt);
                    long[] ws = { st[0] ^ 0xABCDEFL };
                    bar.await();
                    for (int i = 0; i < warm; i++) pc += Common.oneRequest(ws, tb);
                    bar.await();
                    long c = 0; int base = fid * n;
                    t0s[fid] = System.nanoTime();
                    for (int i = 0; i < n; i++) { long t = System.nanoTime(); c += Common.oneRequest(st, tb); lat[base + i] = (int) (System.nanoTime() - t); }
                    t1s[fid] = System.nanoTime(); cs[fid] = c + (pc & 0);
                } catch (Exception e) { throw new RuntimeException(e); } finally { done.countDown(); }
            };
        }
        var ex = threads.equals("virtual") ? Executors.newVirtualThreadPerTaskExecutor() : Executors.newFixedThreadPool(T);
        for (Runnable r : rs) ex.submit(r);
        done.await(); ex.shutdown();
        long a = t0s[0], b = t1s[0]; for (int i = 1; i < T; i++) { a = Math.min(a, t0s[i]); b = Math.max(b, t1s[i]); }
        int N = n * T; Arrays.sort(lat);
        row("W3", "rps", N / ((b - a) / 1e9)); row("W3", "p50_ns", lat[N / 2]); row("W3", "p99_ns", lat[(int) (N * 0.99)]); row("W3", "p999_ns", lat[(int) (N * 0.999)]);
        row("W3", "rss_mb", vmhwmMB()); row("W3", "cpu_s", cpuS());
        if (T == 1) row("W3", "checksum", (double) (cs[0] & 0xffffffffL));
    }

    // ---- W4
    static final class Node { long val; Node c0, c1, c2, c3, parent; }
    static boolean weak;
    static Node build(long i, long N, Node parent) {
        Node n = new Node(); n.val = i; if (weak && parent != null) n.parent = parent;
        long c = 4 * i + 1;
        if (c < N) n.c0 = build(c, N, n); if (c + 1 < N) n.c1 = build(c + 1, N, n); if (c + 2 < N) n.c2 = build(c + 2, N, n); if (c + 3 < N) n.c3 = build(c + 3, N, n);
        return n;
    }
    static long tsum(Node n) {
        long s = n.val; if (weak && n.parent != null) s += n.parent.val;
        if (n.c0 != null) s += tsum(n.c0); if (n.c1 != null) s += tsum(n.c1); if (n.c2 != null) s += tsum(n.c2); if (n.c3 != null) s += tsum(n.c3);
        return s;
    }
    static volatile long sink;
    static void w4(String[] args) throws Exception {
        int T = (int) geti(args, "threads", 1); long N = geti(args, "nodes", 1000000); int cycles = (int) geti(args, "cycles", 6);
        weak = geti(args, "weak", 0) != 0; params = "threads=" + T + ";weak=" + geti(args, "weak", 0);
        double[][] bs = new double[T][cycles], ts = new double[T][cycles];
        Thread[] th = new Thread[T];
        for (int id = 0; id < T; id++) {
            final int fid = id;
            th[id] = Thread.ofPlatform().start(() -> {
                long s = 0;
                for (int c = 0; c < cycles; c++) {
                    long t0 = System.nanoTime(); Node root = build(0, N, null);
                    long t1 = System.nanoTime(); s += tsum(root);
                    long t2 = System.nanoTime(); root = null;
                    bs[fid][c] = (double) (t1 - t0) / N; ts[fid][c] = (double) (t2 - t1) / N;
                }
                sink += s;
            });
        }
        for (Thread t : th) t.join();
        double b = 0, t = 0;
        for (int i = 0; i < T; i++) { double[] x = Arrays.copyOfRange(bs[i], 1, cycles), y = Arrays.copyOfRange(ts[i], 1, cycles); Arrays.sort(x); Arrays.sort(y); b += x[x.length / 2]; t += y[y.length / 2]; }
        b /= T; t /= T;
        row("W4", "build_ns_per_node", b); row("W4", "traverse_ns_per_node", t); row("W4", "free_ns_per_node", 0); row("W4", "cycle_ns_per_node", b + t);
        row("W4", "cpu_ns_per_node_cycle", cpuS() * 1e9 / ((double) N * cycles * T)); row("W4", "rss_mb", vmhwmMB());
    }

    public static void main(String[] args) throws Exception {
        if (args.length < 1) { System.err.println("usage: w3|w4 k=v..."); System.exit(2); }
        String[] rest = Arrays.copyOfRange(args, 1, args.length);
        switch (args[0]) { case "w3" -> w3(rest); case "w4" -> w4(rest); default -> { if (args[0].equals("jmh")) org.openjdk.jmh.Main.main(rest); } }
    }
}
