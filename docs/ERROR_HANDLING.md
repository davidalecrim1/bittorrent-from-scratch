
Below is a **clean, corrected, production-grade rewrite**.
Same intent as the original, but internally consistent, idiomatic, and safer to copy.

---

# Error Handling and Logging in Modern Rust (Practical & Matchable)

Rust error handling is powerful **only if errors preserve meaning and structure**.
This guide shows how to design errors that are **matchable, composable, and debuggable**—without overusing `anyhow` or losing error context.

The core idea is simple:

> **Typed errors inside your application. Dynamic errors only at the boundary.**

---

## Design Principles

1. **Domain logic returns typed errors**
2. **Errors preserve their source**
3. **Matching is explicit, not dynamic**
4. **Logging happens at boundaries**
5. **Context is structured, not stringified**

---

## Define Typed Errors (Correctly)

Use `thiserror` to define **semantic errors with sources**.

```rust
// src/error.rs
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("invalid input")]
    InvalidInput {
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    #[error("database connection failed")]
    DatabaseConnection {
        #[source]
        source: DatabaseError,
    },

    #[error("resource not found: {id}")]
    NotFound {
        id: String,
    },
}
```

Key points:

* Errors **own their causes**
* No `String` as the only information
* The error chain is preserved automatically

---

## Returning Errors from Domain Code

Domain functions return **concrete error types**.

```rust
fn handle_request(req: Request) -> Result<Response, AppError> {
    let user_id = extract_user_id(&req)
        .map_err(|e| AppError::InvalidInput { source: e.into() })?;

    let user = fetch_user(user_id)?;
    Ok(Response::ok(user))
}
```

No logging here.
No `anyhow`.
Just meaning.

---

## Error Matching (The Real Payoff)

Typed errors enable **direct, exhaustive matching**.

```rust
match handle_request(req) {
    Ok(resp) => Ok(resp),

    Err(AppError::NotFound { id }) => {
        Ok(Response::not_found(id))
    }

    Err(AppError::InvalidInput { .. }) => {
        Ok(Response::bad_request())
    }

    Err(e) => Err(e),
}
```

No downcasting.
No guessing.
No runtime type inspection.

---

## Where `anyhow` *Does* Belong

Use `anyhow` **only at application boundaries**:

* `main`
* CLI commands
* Background workers
* HTTP handlers

Example:

```rust
use anyhow::Context;

fn main() -> anyhow::Result<()> {
    run().context("application startup failed")?;
    Ok(())
}
```

If you need to convert from `anyhow` into a typed error, **keep the source**:

```rust
fn load_config(path: &str) -> Result<Config, AppError> {
    let content = std::fs::read_to_string(path)
        .map_err(|e| AppError::InvalidInput { source: e.into() })?;

    toml::from_str(&content)
        .map_err(|e| AppError::InvalidInput { source: e.into() })
}
```

Never call `.to_string()` on errors unless you are **printing**, not **handling**.

---

## Logging: One Place Only

**Rule:**

> Errors are logged at boundaries, not where they occur.

```rust
pub fn http_handler(req: Request) -> Response {
    match handle_request(req) {
        Ok(resp) => resp,
        Err(e) => {
            tracing::error!(error = %e, "request failed");
            Response::internal_error()
        }
    }
}
```

Avoid:

* Logging + returning the error
* Logging inside domain code
* Logging every match arm

---

## Recommended Log Levels

* `error` → request / job failed
* `warn` → degraded behavior
* `info` → lifecycle events
* `debug` → diagnostics
* `trace` → internals

Prefer `tracing` over `log` for structured context.

---

## Layered Errors Scale Better Than One Global Enum

Large systems should avoid a single god-enum.

Instead:

```text
DomainError → ServiceError → AppError
```

Each layer converts errors at its boundary.
This keeps meaning local and prevents enum explosion.

---

## Consolidated Example

```rust
pub async fn handle_user(req: Request) -> Result<Response, AppError> {
    let id = extract_user_id(&req)
        .map_err(|e| AppError::InvalidInput { source: e.into() })?;

    match fetch_user(id).await {
        Ok(user) => Ok(Response::ok(user)),
        Err(AppError::NotFound { id }) => Ok(Response::not_found(id)),
    Err(e) => Err(e),
    }
}
```

---

## Key Takeaways

1. **Typed errors are for control flow**
2. **`anyhow` is for boundaries, not domains**
3. **Never erase error sources**
4. **Match errors, don’t downcast**
5. **Log once, at the edge**
6. **Prefer structured context over strings**

If you violate any of these, Rust will still compile—but your system will rot.
