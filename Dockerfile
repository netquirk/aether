# The production image the prairie runner builds and pushes for the panel target.
#
# The runner compiles the binary in the project's sandbox and copies the
# artefact to `target/debug/aether` -- the path `.dockerignore` keeps in the
# build context -- so the image only has to carry that binary: nothing is
# compiled here and the runtime image stays small.
#
# The container serves the ACP WebSocket API and the deploy contract's health
# probe on the same port; the probe path is `/healthz`.

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates git \
    && rm -rf /var/lib/apt/lists/*

RUN useradd --create-home --uid 10001 aether

COPY --chown=aether:aether target/debug/aether /usr/local/bin/aether

WORKDIR /workspace

USER aether

EXPOSE 8765

ENTRYPOINT ["aether"]
CMD ["server", "--listen", "0.0.0.0:8765"]
