package main

import "sync"

// Same workloads as c/rcm.c. Idiomatic Go: GC'd, pointers passed freely, striped RWMutex table.

const (
	FNV  = 0x100000001b3
	MIX  = 0x9E3779B97F4A7C15
	NSTR = 256
)

func rng(s *uint64) uint64 {
	x := *s
	x ^= x >> 12
	x ^= x << 25
	x ^= x >> 27
	*s = x
	return x * 2685821657736338717
}

// ---- W1/W2
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

type stripe struct {
	mu sync.RWMutex
	_  [40]byte
}
type Table[T any] struct {
	st   [NSTR]stripe
	slot []*T
}

func newTable[T any](n int) *Table[T] { return &Table[T]{slot: make([]*T, n)} }
func (t *Table[T]) get(k int) *T {
	s := &t.st[k&(NSTR-1)]
	s.mu.RLock()
	o := t.slot[k]
	s.mu.RUnlock()
	return o
}
func (t *Table[T]) swap(k int, n *T) *T {
	s := &t.st[k&(NSTR-1)]
	s.mu.Lock()
	o := t.slot[k]
	t.slot[k] = n
	s.mu.Unlock()
	return o
}

// ---- W3 types
type Hdr struct {
	name uint32
	val  [3]uint64
}
type Arr struct {
	n  uint32
	hs [8]Hdr
}
type Buf512 struct{ data [64]uint64 }
type Buf256 struct{ data [32]uint64 }
type Req struct {
	id      uint64
	headers *Arr
	body    *Buf512
}
type Sess struct {
	uid, counter uint64
	data         [5]uint64
}
type Resp struct {
	status uint32
	body   *Buf256
}
type Ctx struct {
	acc  uint64
	sid  uint32
	sess *Sess
	resp *Resp
}

const NSESS = 1024

var gWork = 1

//go:noinline
func newSess(uid, counter uint64) *Sess {
	s := &Sess{uid: uid, counter: counter}
	for i := 0; i < 5; i++ {
		s.data[i] = uid * uint64(i+1)
	}
	return s
}

//go:noinline
func newReq(rs *uint64) *Req {
	rq := &Req{id: rng(rs)}
	a := &Arr{n: 8}
	for j := 0; j < 8; j++ {
		a.hs[j].name = uint32(j)
		for w := 0; w < 3; w++ {
			a.hs[j].val[w] = rng(rs)
		}
	}
	b := &Buf512{}
	for i := 0; i < 64; i++ {
		b.data[i] = rng(rs)
	}
	rq.headers = a
	rq.body = b
	return rq
}

//go:noinline
func newCtx() *Ctx { return &Ctx{} }

//go:noinline
func newResp() *Resp { return &Resp{status: 200} }

//go:noinline
func newBuf256() *Buf256 { return &Buf256{} }

//go:noinline
func layer(k int, rq *Req, cx *Ctx, tb *Table[Sess]) uint64 {
	var r uint64
	switch k {
	case 0:
		h := rq.headers
		cx.acc = h.hs[0].val[0] ^ h.hs[0].val[1]
	case 1:
		cx.acc += rq.id
	case 2:
		h := rq.headers
		for j := 0; j < 8; j++ {
			if h.hs[j].name == 3 {
				cx.acc ^= h.hs[j].val[0]
			}
		}
	case 3:
		cx.acc = cx.acc*MIX + 1
	case 4:
		h := rq.headers
		cx.sid = uint32((h.hs[5].val[0] ^ h.hs[5].val[1] ^ h.hs[5].val[2]) % NSESS)
		cx.acc += uint64(cx.sid)
	case 5:
		s := tb.get(int(cx.sid))
		cx.acc += s.counter
		if (rq.id>>40)%20 == 0 {
			n := newSess(s.uid, s.counter+1)
			tb.swap(int(cx.sid), n)
		}
		cx.sess = s
	case 6:
		cx.acc ^= (rq.id >> 8) & 0xff
	case 7:
		b := rq.body
		a := cx.acc
		for w := 0; w < gWork; w++ {
			for i := 0; i < 64; i++ {
				a = (a ^ b.data[i]) * FNV
			}
		}
		cx.acc = a
	case 8:
		s := cx.sess
		rp := newResp()
		b := newBuf256()
		for i := 0; i < 32; i++ {
			b.data[i] = cx.acc + uint64(i)*s.counter + s.uid
		}
		cx.acc ^= b.data[31]
		rp.body = b
		cx.resp = rp
	case 9:
		rp := cx.resp
		b := rp.body
		a := cx.acc
		for w := 0; w < gWork; w++ {
			for i := 0; i < 32; i++ {
				a = (a ^ b.data[i]) * FNV
			}
		}
		cx.acc = a + uint64(rp.status)
	}
	if k < 9 {
		r = layerFn(k+1, rq, cx, tb)
	} else {
		r = cx.acc
	}
	return r
}

// middleware chains are dispatched through a function value (as in real routers); this also stops
// escape analysis from stack-allocating the request objects.
var layerFn func(int, *Req, *Ctx, *Table[Sess]) uint64

func init() { layerFn = layer }

func oneRequest(rs *uint64, tb *Table[Sess]) uint64 {
	rq := newReq(rs)
	cx := newCtx()
	return layerFn(0, rq, cx, tb)
}
