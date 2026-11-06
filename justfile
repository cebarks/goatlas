fixtures: fixtures-base

fixtures-base:
    #!/bin/sh
    set -eu
    export GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off
    if ! version=$(go version 2>&1); then
        printf '%s\n' "local Go toolchain required: $version" >&2
        exit 1
    fi
    mkdir -p target/fixtures
    printf '%s\n' "$version" > target/fixtures/go-version.txt
    cd tests/fixtures/app
    export GOOS=linux GOARCH=amd64 CGO_ENABLED=0
    go build -mod=readonly -trimpath -buildvcs=false -gcflags='example.com/golang-symbol/fixture=-m=2' -o ../../../target/fixtures/app.unstripped . 2> ../../../target/fixtures/unstripped.compiler.log
    go build -mod=readonly -trimpath -buildvcs=false -gcflags='example.com/golang-symbol/fixture=-m=2' -ldflags='-s -w' -o ../../../target/fixtures/app.stripped . 2> ../../../target/fixtures/stripped.compiler.log

test: fixtures
    cargo test --locked

check: test
    cargo fmt --all -- --check
    cargo clippy --locked --all-targets -- -D warnings
