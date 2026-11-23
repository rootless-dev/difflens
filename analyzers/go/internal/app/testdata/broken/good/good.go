package good

import "example.com/broken/bad"

func Run() int { return twice(bad.Do()) }

func twice(n int) int { return n * 2 }
