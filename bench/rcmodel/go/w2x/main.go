// w2x: comparative W2 driver (same protocol as c/w2sync.c with warm_ms, java rcm.W2X, rust bin w2x).
// 10k-entry shared table, wpct% writes (new entry swapped in), reads walk the entry 3 frames deep, then hash= bytes of
// FNV-1a over a thread-local buffer seeded by the value read. Threads publish op counts every 64 ops; the op-count delta
// over [warm_ms, warm_ms+dur_ms] gives Mops/s.
//
//	impl=rwmutex  striped sync.RWMutex table (as go/common.go)
//	impl=atomic   []atomic.Pointer[Entry]: lock-free load, GC reclaims
package main

import (
	"fmt"
	"os"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

const (
	nent = 10000
	nstr = 256
	hbuf = 16384
)

type Entry struct {
	key, val int64
	payload  [48]byte
	pad      [48]byte
}

func newEntry(key, val int64) *Entry {
	e := &Entry{key: key, val: val}
	for i := range e.payload {
		e.payload[i] = byte(key)
	}
	return e
}

//go:noinline
func walk(e *Entry, d int) int64 {
	if d <= 1 {
		return e.val
	}
	return walk(e, d-1)
}

//go:noinline
func fnv(b []byte, seed uint64) uint64 {
	h := uint64(0xcbf29ce484222325) ^ seed
	for _, c := range b {
		h = (h ^ uint64(c)) * 0x100000001b3
	}
	return h
}

func rng(s *uint64) uint64 {
	x := *s
	x ^= x >> 12
	x ^= x << 25
	x ^= x >> 27
	*s = x
	return x * 2685821657736338717
}

type stripe struct {
	mu sync.RWMutex
	_  [40]byte
}

type table interface {
	get(k int) *Entry
	swap(k int, e *Entry)
}

type rwTable struct {
	st   [nstr]stripe
	slot []*Entry
}

func (t *rwTable) get(k int) *Entry {
	s := &t.st[k&(nstr-1)]
	s.mu.RLock()
	o := t.slot[k]
	s.mu.RUnlock()
	return o
}
func (t *rwTable) swap(k int, e *Entry) {
	s := &t.st[k&(nstr-1)]
	s.mu.Lock()
	t.slot[k] = e
	s.mu.Unlock()
}

type atTable struct{ slot []atomic.Pointer[Entry] }

func (t *atTable) get(k int) *Entry     { return t.slot[k].Load() }
func (t *atTable) swap(k int, e *Entry) { t.slot[k].Store(e) }

type prog struct {
	n atomic.Int64
	_ [120]byte
}

func arg(k, d string) string {
	for _, a := range os.Args[1:] {
		if strings.HasPrefix(a, k+"=") {
			return a[len(k)+1:]
		}
	}
	return d
}
func argi(k string, d int) int {
	v, err := strconv.Atoi(arg(k, strconv.Itoa(d)))
	if err != nil {
		fmt.Fprintln(os.Stderr, "bad", k)
		os.Exit(2)
	}
	return v
}

var gsink atomic.Int64

func main() {
	impl, mode := arg("impl", "rwmutex"), arg("mode", "hot_same")
	T, wpct, hash := argi("threads", 1), argi("wpct", 5), argi("hash", 0)
	warm, dur := argi("warm_ms", 500), argi("dur_ms", 1000)
	if hash < 0 || hash > hbuf {
		fmt.Fprintln(os.Stderr, "hash out of range")
		os.Exit(2)
	}
	var t table
	switch impl {
	case "rwmutex":
		r := &rwTable{slot: make([]*Entry, nent)}
		for i := range r.slot {
			r.slot[i] = newEntry(int64(i), int64(i*5+1))
		}
		t = r
	case "atomic":
		a := &atTable{slot: make([]atomic.Pointer[Entry], nent)}
		for i := range a.slot {
			a.slot[i].Store(newEntry(int64(i), int64(i*5+1)))
		}
		t = a
	default:
		fmt.Fprintln(os.Stderr, "impl=rwmutex|atomic")
		os.Exit(2)
	}
	progs := make([]prog, T)
	var stop atomic.Bool
	var wg sync.WaitGroup
	for id := 0; id < T; id++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			s := uint64(0x9E3779B97F4A7C15) ^ (uint64(id+1) * 0x1234567)
			s2 := s ^ 0x5555
			hb := make([]byte, hbuf)
			for j := range hb {
				hb[j] = byte(rng(&s2))
			}
			hot := 17
			if mode == "hot_diff" {
				hot = (id*601 + 7) % nent
			}
			var sink int64
			for i := int64(0); ; i++ {
				if i&63 == 0 {
					progs[id].n.Store(i)
					if stop.Load() {
						break
					}
				}
				r := rng(&s)
				k := hot
				if mode == "uniform" {
					k = int((r >> 11) % nent)
				}
				r2 := rng(&s)
				if int((r2>>33)%100) < wpct {
					t.swap(k, newEntry(int64(k), int64(r2&0xffff)))
					continue
				}
				v := walk(t.get(k), 3)
				sink += v
				if hash > 0 {
					sink += int64(fnv(hb[:hash], uint64(v)))
				}
			}
			gsink.Add(sink)
		}(id)
	}
	sum := func() int64 {
		var n int64
		for i := range progs {
			n += progs[i].n.Load()
		}
		return n
	}
	time.Sleep(time.Duration(warm) * time.Millisecond)
	c0, w0 := sum(), time.Now()
	time.Sleep(time.Duration(dur) * time.Millisecond)
	c1, w1 := sum(), time.Now()
	stop.Store(true)
	wg.Wait()
	mops := float64(c1-c0) / w1.Sub(w0).Seconds() / 1e6
	params := fmt.Sprintf("threads=%d;mode=%s;wpct=%d;ppct=0;hash=%d", T, mode, wpct, hash)
	fmt.Printf("W2X,go,%s,%s,mops_total,%.6f\n", impl, params, mops)
	fmt.Printf("W2X,go,%s,%s,rss_mb,%.1f\n", impl, params, rssMB())
}

func rssMB() float64 {
	b, err := os.ReadFile("/proc/self/status")
	if err != nil {
		return 0
	}
	for _, l := range strings.Split(string(b), "\n") {
		if strings.HasPrefix(l, "VmHWM:") {
			v, _ := strconv.ParseFloat(strings.TrimSpace(strings.TrimSuffix(strings.TrimSpace(l[6:]), "kB")), 64)
			return v / 1024
		}
	}
	return 0
}
