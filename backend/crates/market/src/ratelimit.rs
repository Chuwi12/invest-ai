//! Token bucket sencillo, uno por proveedor.
//!
//! Las reservas pueden dejar el saldo en negativo: eso es la «cola» de peticiones que esperan su
//! turno. Si la espera supera `max_wait` se devuelve el error y el enrutador prueba otro
//! proveedor en lugar de bloquear la petición.

use std::{sync::Mutex, time::Duration};

use tokio::time::Instant;

pub struct RateLimiter {
    rate: f64,
    burst: f64,
    state: Mutex<State>,
}

struct State {
    tokens: f64,
    last: Instant,
}

impl RateLimiter {
    pub fn new(per_sec: f64, burst: u32) -> Self {
        let burst = f64::from(burst.max(1));
        Self {
            rate: per_sec.max(0.001),
            burst,
            state: Mutex::new(State { tokens: burst, last: Instant::now() }),
        }
    }

    /// Reserva un token. `Err(espera)` si habría que esperar más de `max_wait`.
    pub async fn acquire(&self, max_wait: Duration) -> Result<(), Duration> {
        let wait = {
            // Sección crítica mínima y sin await: no hay riesgo de bloquear el runtime.
            let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
            let now = Instant::now();
            let elapsed = now.duration_since(s.last).as_secs_f64();
            s.tokens = (s.tokens + elapsed * self.rate).min(self.burst);
            s.last = now;
            s.tokens -= 1.0;
            if s.tokens >= 0.0 {
                return Ok(());
            }
            let wait = Duration::from_secs_f64(-s.tokens / self.rate);
            if wait > max_wait {
                s.tokens += 1.0; // devuelve la reserva
                return Err(wait);
            }
            wait
        };
        tokio::time::sleep(wait).await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn burst_then_waits() {
        let rl = RateLimiter::new(2.0, 2);
        let t0 = Instant::now();
        rl.acquire(Duration::from_secs(5)).await.unwrap();
        rl.acquire(Duration::from_secs(5)).await.unwrap();
        assert!(t0.elapsed() < Duration::from_millis(10));
        // El tercero espera medio segundo (2 tokens/s).
        rl.acquire(Duration::from_secs(5)).await.unwrap();
        assert!(t0.elapsed() >= Duration::from_millis(490));
    }

    #[tokio::test(start_paused = true)]
    async fn rejects_when_wait_too_long() {
        let rl = RateLimiter::new(0.2, 1); // 1 cada 5 s
        rl.acquire(Duration::from_secs(1)).await.unwrap();
        let err = rl.acquire(Duration::from_secs(1)).await.unwrap_err();
        assert!(err >= Duration::from_secs(4));
        // La reserva rechazada no consume saldo: tras 5 s ya se puede.
        tokio::time::advance(Duration::from_secs(5)).await;
        rl.acquire(Duration::from_secs(1)).await.unwrap();
    }
}
