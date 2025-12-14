# Error Handling and Logging Best Practices in Modern Rust Applications

Error handling in Rust doesn't have to be verbose. With `anyhow` for error propagation and pattern matching inspired by Go's `errors.Is`, you can build robust applications with clear, actionable logging.

## Error Handling with Anyhow

### Basic Error Propagation

`anyhow` excels at quick error propagation while preserving context:

```rust
use anyhow::{Context, Result};

fn read_config() -> Result<Config> {
    let content = std::fs::read_to_string("config.toml")
        .context("Failed to read config file")?;
    
    toml::from_str(&content)
        .context("Failed to parse config")
}
```

### Go-style Error Matching

To match specific errors like Go's `errors.Is`, use `downcast_ref`:

```rust
use anyhow::{anyhow, Result};
use std::io;

fn process_file(path: &str) -> Result<()> {
    match read_file(path) {
        Ok(data) => handle_data(data),
        Err(e) => {
            // Match specific error types
            if let Some(io_err) = e.downcast_ref::<io::Error>() {
                match io_err.kind() {
                    io::ErrorKind::NotFound => {
                        log::warn!("File not found: {}, using defaults", path);
                        Ok(())
                    }
                    io::ErrorKind::PermissionDenied => {
                        log::error!("Permission denied reading: {}", path);
                        Err(e)
                    }
                    _ => Err(e),
                }
            } else {
                Err(e)
            }
        }
    }
}
```

### Custom Error Types for Matching

For application-specific errors you need to match against:

```rust
use anyhow::{Context, Result};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database connection failed")]
    DatabaseConnection,
    
    #[error("Invalid user input: {0}")]
    InvalidInput(String),
    
    #[error("Resource not found: {0}")]
    NotFound(String),
}

fn handle_request(id: &str) -> Result<Response> {
    match fetch_user(id) {
        Ok(user) => Ok(Response::new(user)),
        Err(e) => {
            if let Some(app_err) = e.downcast_ref::<AppError>() {
                match app_err {
                    AppError::NotFound(_) => {
                        log::info!("User {} not found, returning 404", id);
                        Ok(Response::not_found())
                    }
                    AppError::DatabaseConnection => {
                        log::error!("DB connection failed for user lookup");
                        Err(e)
                    }
                    _ => Err(e),
                }
            } else {
                Err(e)
            }
        }
    }
}
```

## Logging Best Practices

### Clear, Non-Noisy Logging

Good logs tell a story without overwhelming:

```rust
use log::{debug, info, warn, error};

fn process_batch(items: &[Item]) -> Result<()> {
    debug!("Processing batch of {} items", items.len());
    
    for (idx, item) in items.iter().enumerate() {
        match item.validate() {
            Ok(_) => {
                // Only log validation failures, not successes
                debug!("Item {}: validated", idx);
            }
            Err(e) => {
                warn!("Item {}: validation failed: {:#}", idx, e);
                continue;
            }
        }
        
        item.process()
            .with_context(|| format!("Processing item {}", idx))?;
    }
    
    info!("Batch completed: {}/{} items processed", 
          items.len(), items.len());
    Ok(())
}
```

### Log Level Guidelines

- **`error!`**: Actionable failures requiring immediate attention
- **`warn!`**: Degraded operation or recoverable issues
- **`info!`**: Key business events and state changes
- **`debug!`**: Detailed flow for troubleshooting (not in production)

```rust
fn handle_payment(amount: f64) -> Result<()> {
    info!("Payment initiated: ${:.2}", amount);
    
    match process_payment(amount) {
        Ok(txn_id) => {
            info!("Payment successful: txn={}", txn_id);
            Ok(())
        }
        Err(e) => {
            if let Some(payment_err) = e.downcast_ref::<PaymentError>() {
                match payment_err {
                    PaymentError::InsufficientFunds => {
                        warn!("Payment declined: insufficient funds");
                        Err(e)
                    }
                    PaymentError::NetworkTimeout => {
                        error!("Payment processing timeout - manual review needed");
                        Err(e)
                    }
                    _ => {
                        error!("Payment failed: {:#}", e);
                        Err(e)
                    }
                }
            } else {
                error!("Unexpected payment error: {:#}", e);
                Err(e)
            }
        }
    }
}
```

### Structured Context

Use `anyhow`'s context chaining for clear error trails:

```rust
fn sync_data() -> Result<()> {
    let conn = connect_db()
        .context("Database connection failed")?;
    
    let records = fetch_records(&conn)
        .context("Failed to fetch records from database")?;
    
    upload_to_s3(&records)
        .context("S3 upload failed")?;
    
    info!("Data sync completed: {} records", records.len());
    Ok(())
}
```

When this fails, you get a beautiful error chain:

```
Error: S3 upload failed

Caused by:
    0: Network request failed
    1: Connection timeout
```

### Debug Formatting

Use `{:#}` for pretty-printed errors with full context:

```rust
if let Err(e) = risky_operation() {
    error!("Operation failed: {:#}", e);  // Multi-line with context
    debug!("Error details: {:?}", e);      // Full debug output
}
```

## Putting It Together

```rust
use anyhow::{Context, Result};
use log::{debug, info, error};

pub async fn handle_user_request(req: Request) -> Result<Response> {
    debug!("Request received: method={} path={}", req.method(), req.path());
    
    let user_id = extract_user_id(&req)
        .context("Invalid user ID in request")?;
    
    match fetch_user_data(user_id).await {
        Ok(data) => {
            info!("User data retrieved: user_id={}", user_id);
            Ok(Response::ok(data))
        }
        Err(e) => {
            if let Some(db_err) = e.downcast_ref::<DatabaseError>() {
                match db_err {
                    DatabaseError::NotFound => {
                        debug!("User {} not found", user_id);
                        Ok(Response::not_found())
                    }
                    DatabaseError::ConnectionLost => {
                        error!("Database connection lost during user fetch");
                        Err(e)
                    }
                    _ => {
                        error!("Database error: {:#}", e);
                        Err(e)
                    }
                }
            } else {
                error!("Unexpected error fetching user: {:#}", e);
                Err(e)
            }
        }
    }
}
```

## Key Takeaways

1. **Use `downcast_ref`** for Go-style error matching with `anyhow`
2. **Add context liberally** with `.context()` for clear error trails
3. **Log intentionally**: info for events, warn for issues, error for failures
4. **Format with `{:#}`** for readable multi-line errors
5. **Keep logs actionable**: what happened, what needs attention
6. **Debug logs are free in development**, silent in production unless enabled

This approach gives you Go's error handling clarity with Rust's type safety, and logs that developers actually want to read.
