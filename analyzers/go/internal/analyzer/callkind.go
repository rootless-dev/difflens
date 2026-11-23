package analyzer

import "golang.org/x/tools/go/ssa"

// blockKinds decides whether a call site is "always" or "conditional".
//
// The rule: a call is "always" when its SSA block post-dominates the function
// entry with respect to normal returns, that is, every path from the entry to
// a return instruction passes through the block; paths that end in a panic
// are ignored. A deferred call is judged by the block of its defer statement,
// because once registered it runs on every normal return. A function with no
// reachable return (an endless loop) uses its loop back-edges as exits, so
// "always" then means "on every iteration". go statements and functions
// handed to known async APIs are "async" (decided by the caller of this type);
// everything else is "conditional".
type blockKinds struct {
	fn    *ssa.Function
	exits map[*ssa.BasicBlock]bool
	cache map[*ssa.BasicBlock]bool
}

func newBlockKinds(fn *ssa.Function) *blockKinds {
	k := &blockKinds{fn: fn, exits: map[*ssa.BasicBlock]bool{}, cache: map[*ssa.BasicBlock]bool{}}
	for _, b := range fn.Blocks {
		if n := len(b.Instrs); n > 0 {
			if _, ok := b.Instrs[n-1].(*ssa.Return); ok {
				k.exits[b] = true
			}
		}
	}
	if len(k.exits) == 0 {
		for _, b := range fn.Blocks {
			for _, succ := range b.Succs {
				if succ.Dominates(b) {
					k.exits[b] = true
				}
			}
		}
	}
	return k
}

// always reports whether every path from the entry to an exit goes through b.
func (k *blockKinds) always(b *ssa.BasicBlock) bool {
	if v, ok := k.cache[b]; ok {
		return v
	}
	v := k.compute(b)
	k.cache[b] = v
	return v
}

func (k *blockKinds) compute(b *ssa.BasicBlock) bool {
	if len(k.exits) == 0 || len(k.fn.Blocks) == 0 {
		return true
	}
	entry := k.fn.Blocks[0]
	if b == entry {
		return true
	}
	seen := map[*ssa.BasicBlock]bool{entry: true}
	queue := []*ssa.BasicBlock{entry}
	for len(queue) > 0 {
		cur := queue[0]
		queue = queue[1:]
		if k.exits[cur] {
			return false // reached an exit without passing through b
		}
		for _, succ := range cur.Succs {
			if succ != b && !seen[succ] {
				seen[succ] = true
				queue = append(queue, succ)
			}
		}
	}
	return true
}
