# hw-monitor

A Yoke Plugin that publishes the host's CPU and memory, once a second, on two streams.

Nothing is granted by declaring it: an operator grants `stream.hw.cpu.publish` and `stream.hw.memory.publish`.

Take it into an application with `yoke-compose add plugin https://github.com/yoke-project/yoke-plugin-hw-monitor@<tag>`.
Build it with `yoke-compose build`, and try it from `try/`.

Licensed under the Apache License 2.0.
