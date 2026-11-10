fixtures: fixtures-base fixtures-pie fixtures-noinline fixtures-oldgo

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

fixtures-pie:
    #!/bin/sh
    set -eu
    export GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off
    go version
    mkdir -p target/fixtures
    cd tests/fixtures/app
    export GOOS=linux GOARCH=amd64 CGO_ENABLED=0
    go build -mod=readonly -trimpath -buildvcs=false -buildmode=pie -ldflags='-s -w' -o ../../../target/fixtures/app.pie.stripped .

# app.word40.stripped exercises the pre-1.27 moduledata layout (gofunc at
# word 40) using a pinned mise go1.25.7 toolchain. The install must already
# exist (mise where never downloads); the go1.27.3 fixtures stay untouched.
fixtures-oldgo:
    #!/bin/sh
    set -eu
    mise where go@1.25.7 >/dev/null
    mkdir -p target/fixtures
    cd tests/fixtures/app
    mise exec go@1.25.7 -- env GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off GOOS=linux GOARCH=amd64 CGO_ENABLED=0 go build -mod=readonly -trimpath -buildvcs=false -gcflags='example.com/golang-symbol/fixture=-m=2' -ldflags='-s -w' -o ../../../target/fixtures/app.word40.stripped . 2> ../../../target/fixtures/word40.compiler.log
    mise exec go@1.25.7 -- go version > ../../../target/fixtures/word40-go-version.txt

# app.noinline compiles the fixture package with inlining disabled so its
# own helpers stay standalone and their inline trees are genuinely empty.
fixtures-noinline:
    #!/bin/sh
    set -eu
    export GOTOOLCHAIN=local GOPROXY=off GOSUMDB=off
    go version
    mkdir -p target/fixtures
    cd tests/fixtures/app
    export GOOS=linux GOARCH=amd64 CGO_ENABLED=0
    go build -mod=readonly -trimpath -buildvcs=false -gcflags='example.com/golang-symbol/fixture=-l' -ldflags='-s -w' -o ../../../target/fixtures/app.noinline . 2> ../../../target/fixtures/noinline.compiler.log

test: fixtures
    cargo test --locked

patch-check:
    #!/bin/sh
    set -eu
    export CARGO_TARGET_DIR="{{justfile_directory()}}/target/vendor-unstrip"
    cargo fmt --manifest-path vendor/unstrip/Cargo.toml --all -- --check
    cargo test --manifest-path vendor/unstrip/Cargo.toml --locked --lib
    cargo clippy --manifest-path vendor/unstrip/Cargo.toml --locked --lib -- -D warnings

check: test patch-check
    cargo fmt --all -- --check
    cargo clippy --locked --all-targets -- -D warnings
