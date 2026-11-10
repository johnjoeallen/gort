package main

import (
	"bufio"
	"fmt"
	"os"
	"runtime"
	"slices"
	"strconv"
	"strings"
	"sync"
	"syscall"
	"time"
)

var params = ""
var impl = "go"
var variant = "Go-gc"

func row(w, metric string, v float64) {
	fmt.Printf("%s,%s,%s,%s,%s,%.12g\n", w, impl, variant, params, metric, v)
}
func geti(args []string, k string, d int64) int64 {
	for _, a := range args {
		if strings.HasPrefix(a, k+"=") {
			n, _ := strconv.ParseInt(a[len(k)+1:], 10, 64)
			return n
		}
	}
	return d
}
func gets(args []string, k, d string) string {
	for _, a := range args {
		if strings.HasPrefix(a, k+"=") {
			return a[len(k)+1:]
		}
	}
	return d
}
func vmhwmMB() float64 {
	f, err := os.Open("/proc/self/status")
	if err != nil {
		return 0
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	for sc.Scan() {
		if strings.HasPrefix(sc.Text(), "VmHWM:") {
			var kb float64
			fmt.Sscanf(strings.TrimSpace(sc.Text()[6:]), "%f", &kb)
			return kb / 1024
		}
	}
	return 0
}
func cpuS() float64 {
	var r syscall.Rusage
	syscall.Getrusage(0, &r)
	return float64(r.Utime.Sec+r.Stime.Sec) + float64(r.Utime.Usec+r.Stime.Usec)/1e6
}

func w3(args []string) {
	T := int(geti(args, "workers", 1))
	total := int(geti(args, "requests", 5000000))
	warm := int(geti(args, "warmup", 300000))
	gWork = int(geti(args, "work", 1))
	tbm := gets(args, "table", "shared")
	variant = gets(args, "variant", variant)
	params = fmt.Sprintf("workers=%d;table=%s;work=%d", T, tbm, gWork)
	private := tbm == "private"
	mk := func() *Table[Sess] {
		t := newTable[Sess](NSESS)
		for i := 0; i < NSESS; i++ {
			t.slot[i] = newSess(uint64(i), uint64(i*7))
		}
		return t
	}
	var shared *Table[Sess]
	if !private {
		shared = mk()
	}
	n := total / T
	lat := make([]uint32, n*T)
	cs := make([]uint64, T)
	t0s := make([]int64, T)
	t1s := make([]int64, T)
	var ready, start sync.WaitGroup
	ready.Add(T)
	start.Add(1)
	var done sync.WaitGroup
	done.Add(T)
	var mid sync.WaitGroup // second barrier after warm-up
	mid.Add(T)
	var midGo sync.WaitGroup
	midGo.Add(1)
	base := time.Now()
	for id := 0; id < T; id++ {
		go func(id int) {
			defer done.Done()
			rs := uint64(0x9E3779B97F4A7C15) ^ (uint64(id+1) * 0x1234567)
			tb := shared
			if private {
				tb = mk()
			}
			ready.Done()
			start.Wait()
			ws := rs ^ 0xABCDEF
			for i := 0; i < warm; i++ {
				oneRequest(&ws, tb)
			}
			mid.Done()
			midGo.Wait()
			var c uint64
			l := lat[id*n : (id+1)*n]
			t0s[id] = int64(time.Since(base))
			for i := 0; i < n; i++ {
				t := time.Since(base)
				c += oneRequest(&rs, tb)
				l[i] = uint32(time.Since(base) - t)
			}
			t1s[id] = int64(time.Since(base))
			cs[id] = c
		}(id)
	}
	ready.Wait()
	start.Done()
	mid.Wait()
	midGo.Done()
	done.Wait()
	a, b := t0s[0], t1s[0]
	for i := 1; i < T; i++ {
		a = min(a, t0s[i])
		b = max(b, t1s[i])
	}
	N := n * T
	slices.Sort(lat)
	row("W3", "rps", float64(N)/(float64(b-a)/1e9))
	row("W3", "p50_ns", float64(lat[N/2]))
	row("W3", "p99_ns", float64(lat[int(float64(N)*0.99)]))
	row("W3", "p999_ns", float64(lat[int(float64(N)*0.999)]))
	row("W3", "rss_mb", vmhwmMB())
	row("W3", "cpu_s", cpuS())
	if T == 1 {
		row("W3", "checksum", float64(cs[0]&0xffffffff))
	}
}

// ---- W4
type Node struct {
	val    int64
	ch     [4]*Node
	parent *Node
}

var gWeak bool

func build(i, N int64, parent *Node) *Node {
	n := &Node{val: i}
	if gWeak && parent != nil {
		n.parent = parent
	}
	for c := int64(0); c < 4; c++ {
		ci := 4*i + 1 + c
		if ci < N {
			n.ch[c] = build(ci, N, n)
		}
	}
	return n
}

//go:noinline
func tsum(n *Node) int64 {
	s := n.val
	if gWeak && n.parent != nil {
		s += n.parent.val
	}
	for c := 0; c < 4; c++ {
		if n.ch[c] != nil {
			s += tsum(n.ch[c])
		}
	}
	return s
}

var sink int64

func w4(args []string) {
	T := int(geti(args, "threads", 1))
	N := geti(args, "nodes", 1000000)
	cycles := int(geti(args, "cycles", 6))
	gWeak = geti(args, "weak", 0) != 0
	variant = gets(args, "variant", variant)
	params = fmt.Sprintf("threads=%d;weak=%d", T, geti(args, "weak", 0))
	bs := make([][]float64, T)
	ts := make([][]float64, T)
	var wg sync.WaitGroup
	for id := 0; id < T; id++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			var s int64
			for c := 0; c < cycles; c++ {
				t0 := time.Now()
				root := build(0, N, nil)
				t1 := time.Now()
				s += tsum(root)
				t2 := time.Now()
				root = nil
				bs[id] = append(bs[id], float64(t1.Sub(t0))/float64(N))
				ts[id] = append(ts[id], float64(t2.Sub(t1))/float64(N))
			}
			sink += s
		}(id)
	}
	wg.Wait()
	med := func(x []float64) float64 { y := slices.Clone(x[1:]); slices.Sort(y); return y[len(y)/2] }
	var b, t float64
	for i := 0; i < T; i++ {
		b += med(bs[i])
		t += med(ts[i])
	}
	b /= float64(T)
	t /= float64(T)
	row("W4", "build_ns_per_node", b)
	row("W4", "traverse_ns_per_node", t)
	row("W4", "free_ns_per_node", 0) // no explicit free phase: reclamation cost is inside build (GC) and cpu_ns
	row("W4", "cycle_ns_per_node", b+t)
	row("W4", "cpu_ns_per_node_cycle", cpuS()*1e9/(float64(N)*float64(cycles)*float64(T)))
	row("W4", "rss_mb", vmhwmMB())
}

func main() {
	if len(os.Args) < 2 {
		fmt.Fprintln(os.Stderr, "usage: rcm w3|w4 k=v...")
		os.Exit(2)
	}
	_ = runtime.GOMAXPROCS(0)
	switch os.Args[1] {
	case "w3":
		w3(os.Args[2:])
	case "w4":
		w4(os.Args[2:])
	}
}
