#!/bin/bash
# Build the UTF-8 reference image locally (it is not pushed anywhere).
#   tools/utf8-image/build.sh [mruby tag]     default: 4.1.0-rc2
#   JOBS=<n> tools/utf8-image/build.sh …      rake's parallel jobs (default: nproc inside the build)
# Produces kishima/mruby:<tag>-utf8 (see Dockerfile next to this script).
set -eu
TAG="${1:-4.1.0-rc2}"
cd "$(dirname "$0")"
docker build --platform linux/amd64 \
  --build-arg MRUBY_VER="$TAG" \
  ${JOBS:+--build-arg JOBS="$JOBS"} \
  -t "kishima/mruby:${TAG}-utf8" .
docker run --rm "kishima/mruby:${TAG}-utf8" /bin/sh -c \
  'mruby --version && mruby -e "p __ENCODING__, \"日本語\".length, \"日本語\".size"'
