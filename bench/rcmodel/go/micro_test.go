package main

import (
	"fmt"
	"sync/atomic"
	"testing"
)

var gsink int64

func BenchmarkW1(b *testing.B) {
	var pool [64]*Entry
	for i := range pool {
		pool[i] = newEntry(int64(i), int64(i*3+1))
	}
	for _, d := range []int{1, 4, 16} {
		b.Run(fmt.Sprintf("depth=%d", d), func(b *testing.B) {
			var s int64
			for i := 0; i < b.N; i++ {
				s += walk(pool[i&63], d)
			}
			gsink = s
		})
	}
}

func BenchmarkW2(b *testing.B) {
	const nent = 10000
	for _, mode := range []string{"uniform", "hot_same", "hot_diff"} {
		b.Run("mode="+mode, func(b *testing.B) {
			t := newTable[Entry](nent)
			for i := 0; i < nent; i++ {
				t.slot[i] = newEntry(int64(i), int64(i*5+1))
			}
			var ctr atomic.Int64
			b.RunParallel(func(pb *testing.PB) {
				id := ctr.Add(1) - 1
				s := uint64(0x9E3779B97F4A7C15) ^ (uint64(id+1) * 0x1234567)
				hot := 17
				if mode == "hot_diff" {
					hot = int((id*601 + 7) % nent)
				}
				var sink int64
				for pb.Next() {
					r := rng(&s)
					k := hot
					if mode == "uniform" {
						k = int((r >> 11) % nent)
					}
					r2 := rng(&s)
					if (r2>>33)%100 >= 5 {
						e := t.get(k)
						sink += walk(e, 3)
					} else {
						t.swap(k, newEntry(int64(k), int64(r2&0xffff)))
					}
				}
				atomic.AddInt64(&gsink, sink)
			})
		})
	}
}
