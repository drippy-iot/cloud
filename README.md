# Drippy Cloud

## Environment Variables

**Name** | **Description** | **Required** | **Default**
-------- | --------------- | :----------: | ----------:
`PORT` | Network port to bind to when listening for new connections. | &#x2714; |
`RUST_LOG` | Log verbosity level. See the [`env_logger`] crate documentation. | &#x274c; | `error`
`RUST_LOG_STYLE` | Whether to enable ANSI colors in the output. See the [`env_logger`] crate documentation. | &#x274c; | `auto`

[`env_logger`]: https://docs.rs/env_logger/latest/env_logger/

## Automation Scripts

To automate common tasks, we use the [Just](https://just.systems/) task runner. See the [documentation](https://just.systems/man/en/chapter_1.html) for more information.

```bash
# Create the `/data` folder and start the database daemon in one step.
just init

# This is equivalent to running these two tasks.
just initdb
just db
```

```bash
# Initialize and instantiate the template in one step.
just instantiate

# This is equivalent to running these two tasks.
just template
just create
```

```bash
# Connect to the database using a `psql` shell.
just shell
```


```bash
# Drop the instantiated database and recreate a new one from the template.
just respawn

# This is equivalent to running these two tasks.
just drop
just create
```

```bash
# Nuke the entire `/data` folder and reinitialize it.
just reset

# This is equivalent to running these two tasks.
just nuke
just init
```
