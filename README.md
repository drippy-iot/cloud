# Drippy Cloud

## Environment Variables

**Name** | **Description** | **Required** | **Default**
-------- | --------------- | :----------: | ----------:
`PORT` | Network port to bind to when listening for new connections. | &#x2714; |
`RUST_LOG` | Log verbosity level. See the [`env_logger`] crate documentation. | &#x274c; | `error`
`RUST_LOG_STYLE` | Whether to enable ANSI colors in the output. See the [`env_logger`] crate documentation. | &#x274c; | `auto`

[`env_logger`]: https://docs.rs/env_logger/latest/env_logger/
