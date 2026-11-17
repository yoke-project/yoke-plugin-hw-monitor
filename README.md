# hw-monitor

A Yoke Plugin that watches the host's load: CPU, memory and pressure as streams, a snapshot, and two alarms.

- Three streams, while each flows: `hw.cpu`, `hw.memory`, and `hw.pressure` — the load average and the
  kernel's pressure figures, saying `"available": false` where the kernel has none.
- The question `hw.snapshot`: the latest of each, without opening a stream.
- Two occurrences: `hw.memory.low` (severity 50) and `hw.cpu.saturated` (severity 30), each once its
  threshold has held long enough, and again only after the figure has come back past it.

Every sample states `schema`, `at_ms` and `sequence`: the count of its stream's readings since the unit
started, whether or not the stream flowed. **An interface that attaches late** subscribes to a stream, then
asks `hw.snapshot`, whose readings carry theirs, and drops what the stream brings at or below them.

The application sets among the unit's arguments
`--interval-ms` (1000), `--memory-low-percent` (10), `--cpu-saturated-percent` (95) and `--for-seconds` (30).

Nothing is granted by declaring it: an operator grants each `stream.*.publish`, `query.hw.snapshot.answer`
and each `event.*.report`.


Take it into an application with `yoke-compose add plugin https://github.com/yoke-project/yoke-plugin-hw-monitor@<tag>`.
Build it with `yoke-compose build`, and try it from `try/`.

Licensed under the Apache License 2.0.
