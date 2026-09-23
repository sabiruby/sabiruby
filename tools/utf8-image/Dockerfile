# The UTF-8 reference mruby: the same tree as kishima/mruby:<tag>, built with
# MRB_UTF8_STRING, so that `String` counts characters. tools/fixtures.sh and
# tools/custom.sh run the `-utf8` expectations against it. Local only (not pushed).
#
#   tools/utf8-image/build.sh [mruby tag]      (default 4.1.0-rc2)
#
# Written down on 2026-09-23 from `docker history kishima/mruby:4.1.0-rc-utf8`, whose
# recipe was not in the repository; the steps (sed into build_config/utf8.rb, MRUBY_CONFIG,
# rake && rake install, the __ENCODING__ check) are that image's. The base and the clone
# follow the book's tools/gc-images/gc-experiments.Dockerfile.
FROM ruby:3.2.2-bookworm
ARG MRUBY_VER
# rake's parallel jobs. Unset: the build machine's core count (`nproc`), which only
# changes how long the build takes, not what it builds.
ARG JOBS

RUN apt-get update && apt-get install -y \
    build-essential \
    git

RUN git clone https://github.com/mruby/mruby.git /usr/src/mruby-utf8
WORKDIR /usr/src/mruby-utf8
RUN git checkout $MRUBY_VER
# default.rb plus one define; the grep fails the build if default.rb stops matching the sed.
RUN sed "s/  conf.gembox 'default'/  conf.gembox 'default'\n  conf.cc.defines << 'MRB_UTF8_STRING'/" build_config/default.rb > build_config/utf8.rb \
 && grep -n "MRB_UTF8_STRING" build_config/utf8.rb
ENV MRUBY_CONFIG=/usr/src/mruby-utf8/build_config/utf8.rb
RUN rake -j"${JOBS:-$(nproc)}" && rake install
RUN mruby --version && mruby -e 'p __ENCODING__, "日本語".length, "日本語".size'
