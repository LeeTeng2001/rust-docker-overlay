Run a test container & enter target container with a cloned copy of viewed namespace

```bash
$ docker run --rm --name test-postgres -e POSTGRES_PASSWORD=mysecretpassword -d postgres
$ cargo build && sudo ./target/debug/rust-ns-overlay --id <container_id>
```
