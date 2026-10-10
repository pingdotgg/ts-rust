# The exit rule of the goport runs that gate.sh (measure, determinism) and the sweeps (sweep.sh,
# sweep-wide.sh, sweep-extra2.sh, sweep-hono-runtime.sh) judge against saved Go output. Those stages do
# not run Go, so they cannot know Go's exit in the same run: the Go pin of the run decides. Source this
# file after the pin is set (GOPORT_PIN_ACTIVE, scripts/upstream/pin.py); gate.sh records that pin as
# the manifest's upstreamPin. It sets and exports GOPORT_MAX_EXIT (gate.sh's Python reads it) and
# defines incomplete().
#
# EXIT2_PINS lists the Go pins at which tsgo exits 2 for diagnostics under --noEmit (tsgo #4407,
# bbdf7a24b; the first pin is 16c25522e123, bump B). A later pin bump adds its pin here: 673a5f17d713
# (bump C, microsoft/TypeScript tsc/; its oracle exits 2 for the diagnostics of corpus-diag 07615 and 11451) and
# fed0bf24149f (bump D; its oracle exits 2 for a TS2322 file and for the prisma variant.client diagnostics).
# - At any other pin, and with no pin, the old rule holds byte for byte: a run is complete with exit 0
#   or 1 and no "unported" line on stderr (GOPORT_MAX_EXIT 1).
# - At a pin in EXIT2_PINS, exit 2 is complete too, and a Go "panic: " line on stderr makes a run
#   incomplete, because goport exits 2 for a kept Go panic (GOPORT_MAX_EXIT 2).
# Exits over GOPORT_MAX_EXIT are a crash (70: unported code or a Rust panic; 124: timeout; 128+N: a
# signal). The stages that run Go in the same run (emit, corpus-diag, corpus-emit, typesyms) keep their
# own rules and do not read this file.
EXIT2_PINS="16c25522e123 673a5f17d713 fed0bf24149f"

GOPORT_MAX_EXIT=1
for exit2_pin in $EXIT2_PINS; do
  if [ "${GOPORT_PIN_ACTIVE:-}" = "$exit2_pin" ]; then GOPORT_MAX_EXIT=2; fi
done
export GOPORT_MAX_EXIT

# incomplete <exit> <stderr file>: true when a goport run did not complete under the rule above.
incomplete() {
  [ "$1" -gt "$GOPORT_MAX_EXIT" ] || grep -q "^unported" "$2" || { [ "$GOPORT_MAX_EXIT" = 2 ] && grep -q "^panic: " "$2"; }
}
