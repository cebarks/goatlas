package main

import (
	"fmt"
	"os"
	"reflect"

	"example.com/goatlas/fixturedep"
)

//go:noinline
func directKeep(x int) int { return x + fixturedep.Value() }

func inlineOnly(x int) int { return x*x + 7 }

// twoInlineCalls is the repeated-inlining control: its body calls the
// tiny helper twice, so a correct decoder must report two distinct
// records inside one containing function instead of collapsing them.
//go:noinline
func twoInlineCalls(x int) int { return inlineOnly(x) + inlineOnly(x+1) }

// mixed is used both directly (so the compiler inlines it into main)
// and as a function value (which forces the standalone symbol to be
// retained), making it appear in both the function and inline arrays.
func mixed(x int) int { return x*3 + 1 }

//go:noinline
func callMixed(f func(int) int, x int) int { return f(x) }

func deadUnused() int { return 99 }

type ReflectTarget struct{}

//go:noinline
func (*ReflectTarget) ReflectionOnly(x int) int { return x + 23 }

func (*ReflectTarget) metadataOnly() int { return 42 }

func main() {
	if marker := os.Getenv("GOLANG_SYMBOL_EXECUTION_SENTINEL"); marker != "" {
		_ = os.WriteFile(marker, []byte("executed"), 0600)
	}
	x := inlineOnly(len(os.Args))
	two := twoInlineCalls(x)
	byValue := callMixed(mixed, x)
	fmt.Println(directKeep(x), two, mixed(x), byValue)
	method := reflect.ValueOf(&ReflectTarget{}).MethodByName(os.Getenv("SYMBOL_METHOD"))
	if method.IsValid() {
		fmt.Println(method.Call([]reflect.Value{reflect.ValueOf(x)}))
	}
}
