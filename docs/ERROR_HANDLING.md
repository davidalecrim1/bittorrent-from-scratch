# Error Handling and Logging Best Practices in Modern Rust Applications

Error handling in Rust doesn’t have to be verbose — but the way you structure your errors *does* matter for clarity, matchability, and long-term maintainability. Rather than relying on `anyhow::Error` everywhere, define a central set of custom application errors in your project (e.g., in `src/error.rs`) and return them as concrete types. Use `anyhow` selectively to add contextual information where it’s useful for logs or debugging. ([Rust for C Programmers][1])

---

## Centralized Custom Errors (in `src/error.rs`)

Define an enum representing meaningful error cases for your app. Making this the error type of your `Result`s lets callers use **normal pattern matching**, giving you clear control flow:

```rust
// src/error.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("invalid input: {0}")]
    InvalidInput(String),

    #[error("database connection failed")]
    DatabaseConnection(#[from] DatabaseError),

    #[error("resource not found: {0}")]
    NotFound(String),

    // …other domain cases
}
```

Functions then return:

```rust
fn handle_request(...) -> Result<Response, AppError> { /* ... */ }
```

This structure gives you exact error variants to match on later — something `anyhow::Error` *erases* by design. ([DEV Community][2])

---

## Why Custom Errors + `anyhow` Works Better Together

Rust’s standard guidance for error handling suggests:

* Use **custom typed errors** for fine-grained control and handling.
* Use `anyhow` for **application top levels** or when you want to add additional context to errors before handling them. ([Rust for C Programmers][1])

Custom types let you write:

```rust
match do_work() {
    Ok(_) => …,
    Err(AppError::NotFound(id)) => {
        log::info!("thing {id} not found");
        // handle case
    }
    Err(AppError::InvalidInput(msg)) => …,
    Err(e) => …,
}
```

This is clearer and more efficient than downcasting a dynamic `anyhow::Error` every time. ([Rust for C Programmers][1])

You *can* still use `anyhow` helpers to build context before converting to your error type:

```rust
use anyhow::Context;

fn load_config(path: &str) -> Result<Config, AppError> {
    let content = std::fs::read_to_string(path)
        .context("reading config")
        .map_err(|e| AppError::InvalidInput(e.to_string()))?;

    toml::from_str(&content).map_err(|e| AppError::InvalidInput(e.to_string()))
}
```

The context improves debugging output, but the canonical return type remains your custom `AppError`. ([DEV Community][2])

---

## Error Matching and Handling

With custom error types, error matching is direct and ergonomic:

```rust
match handle_request(&req) {
    Ok(resp) => Ok(resp),
    Err(AppError::NotFound(resource)) => {
        log::warn!("Resource {resource} not found");
        Ok(Response::not_found())
    }
    Err(AppError::DatabaseConnection(_)) => {
        log::error!("DB connection failed");
        Err(AppError::DatabaseConnection(...))
    }
    Err(other) => {
        log::error!("Unexpected error: {other:?}");
        Err(other)
    }
}
```

This avoids pulling types out of an `anyhow::Error` at every call site. ([Rust for C Programmers][1])

---

## Logging Best Practices

### Clear, Intentional Logging

Logs should tell *why* something failed without noise:

```rust
use log::{debug, info, warn, error};

fn process_batch(items: &[Item]) -> Result<(), AppError> {
    debug!("Processing {} items", items.len());

    for (i, item) in items.iter().enumerate() {
        match item.validate() {
            Ok(_) => debug!("Item {} validated", i),
            Err(e) => {
                warn!("Item {} validation failed: {e:?}", i);
                continue;
            }
        }

        // Add context without changing underlying type
        item.process()
            .map_err(|e| AppError::InvalidInput(format!("processing {}: {e}", i)))?;
    }

    info!("Batch completed: {} items", items.len());
    Ok(())
}
```

Disable verbose logs (e.g., `debug!`) in production builds unless explicitly enabled.

---

### Recommended Log Levels

* **`error!`**: Failures the app can’t recover from
* **`warn!`**: Recoverable issues or degraded operation
* **`info!`**: High-level state changes and decisions
* **`debug!`**: Detailed diagnostics during development ([Compile N Run][3])

---

## Structured Context

Providing context helps trace failures:

```rust
fn sync_data() -> Result<(), AppError> {
    let conn = connect_db().map_err(|e| {
        AppError::DatabaseConnection(e)
    })?;

    let records = fetch_records(&conn)?;
    upload_to_s3(&records)
        .map_err(|e| AppError::InvalidInput(format!("upload failed: {e}")))?;
    
    info!("Data sync finished: {} records", records.len());
    Ok(())
}
```

This pattern surfaces domain meaning while keeping error types explicit. ([Rust for C Programmers][1])

---

## Consolidated Example

```rust
pub async fn handle_user_request(req: Request) -> Result<Response, AppError> {
    let user_id = extract_user_id(&req)
        .map_err(|_| AppError::InvalidInput("invalid user id".into()))?;

    match fetch_user_data(user_id).await {
        Ok(data) => Ok(Response::ok(data)),
        Err(AppError::NotFound(_)) => Ok(Response::not_found()),
        Err(e) => {
            error!("Error fetching user: {e:?}");
            Err(e)
        }
    }
}
```

Here we match *specific* error cases clearly and log appropriately.

---

## Key Takeaways

1. **Use explicit error types** (`Result<T, AppError>`) for domain logic so you can pattern-match error cases directly. ([Rust for C Programmers][1])
2. **Use `anyhow` primarily for adding context** and error propagation helpers, not as the main error type everywhere. ([DEV Community][2])
3. **Centralize error definitions** (e.g., in `src/error.rs`) so they reflect the actual failure modes your app handles. ([Compile N Run][3])
4. **Match errors directly** rather than downcasting everywhere — this is more ergonomic and expressive.
5. **Log intentionally** according to severity levels — `error`, `warn`, `info`, `debug`. ([Compile N Run][3])

This approach balances Rust’s compile-time guarantees with clear, actionable runtime error handling and readable logs. ([Rust for C Programmers][1])

---
