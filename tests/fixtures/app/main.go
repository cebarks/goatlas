package main

import (
	"fmt"
	"os"
	"reflect"

	"example.com/golang-symbol/fixturedep"
)

//go:noinline
func directKeep(x int) int { return x + fixturedep.Value() }

func inlineOnly(x int) int { return x*x + 7 }

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
	fmt.Println(directKeep(x))
	method := reflect.ValueOf(&ReflectTarget{}).MethodByName(os.Getenv("SYMBOL_METHOD"))
	if method.IsValid() {
		fmt.Println(method.Call([]reflect.Value{reflect.ValueOf(x)}))
	}
}
