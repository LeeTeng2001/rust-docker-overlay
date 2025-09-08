# rust ns overlay [![Build Status][ci-img]][ci] [![Coverage Status][cov-img]][cov] [![Release][release-img]][release]

A useful container debug utility built with rust. It enables user to quickly build rootfs from docker image and install any familiar tools like `htop` & use it in container namespace.

## Features overview

* __[Use any rootfs](#specify-a-different-rootfs)__: Choose your favourite debug distro, default to debian. By default it'll persist any modification made to the rootfs and let you reuse the same rootfs across multiple session/containers.
* __Debug in container namespace__: After execution you'll enter all container namespace except `mount` which is located at `/mnt/container` to avoid polluting rootfs path

## Demo

![demo](./assets/demo.gif)

## Quickstart

Download binary

```bash
$ wget https://github.com/LeeTeng2001/rust-docker-overlay/releases/download/v1.0/rust-ns-overlay-linux-x86_64-gnu.tar.gz
$ tar xvf rust-ns-overlay-linux-x86_64-gnu.tar.gz
$ ./rust-ns-overlay --version
```

Run a test container & enter target container with a cloned copy of viewed namespace

```bash
$ docker run --rm --name test-postgres -e POSTGRES_PASSWORD=mysecretpassword -d postgres
$ sudo ./rust-ns-overlay <container_id>
```

### Specify a different rootfs


```bash
$ sudo ./rust-ns-overlay <container_id> --image ubuntu:latest
```


[ci-img]: https://github.com/LeeTeng2001/rust-docker-overlay/actions/workflows/ci.yaml/badge.svg
[ci]: https://github.com/LeeTeng2001/rust-docker-overlay/actions/workflows/ci.yaml
[cov-img]: https://codecov.io/gh/LeeTeng2001/rust-docker-overlay/graph/badge.svg?token=464MN13408
[cov]: https://codecov.io/gh/LeeTeng2001/rust-docker-overlay
[release-img]: https://img.shields.io/github/v/release/LeeTeng2001/rust-docker-overlay?sort=semver
[release]: https://github.com/LeeTeng2001/rust-docker-overlay/releases/latest