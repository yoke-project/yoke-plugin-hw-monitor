# hw-monitor

A Yoke Plugin that publishes the host's CPU and memory, once a second, on two streams.

- Three streams, while each flows: `hw.cpu`, `hw.memory`, and `hw.pressure` — the load average and the
  kernel's pressure figures, saying `"available": false` where the kernel has none.
- The question `hw.snapshot`: the latest of each, without opening a stream.
- Two occurrences: `hw.memory.low` (severity 50) and `hw.cpu.saturated` (severity 30), each once its
  threshold has held long enough, and again only after the figure has come back past it.

Every sample states `schema` and `at_ms`. The application sets among the unit's arguments
`--interval-ms` (1000), `--memory-low-percent` (10), `--cpu-saturated-percent` (95) and `--for-seconds` (30).

Nothing is granted by declaring it: an operator grants each `stream.*.publish`, `query.hw.snapshot.answer`
and each `event.*.report`.


Take it into an application with `yoke-compose add plugin https://github.com/yoke-project/yoke-plugin-hw-monitor@<tag>`.
Build it with `yoke-compose build`, and try it from `try/`.

Licensed under the Apache License 2.0.
