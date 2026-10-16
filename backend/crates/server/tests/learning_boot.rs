//! El aprendizaje es opcional: un fallo al activar los pesos nunca impide arrancar. Binario de
//! pruebas propio porque toca el estado global de `ai::scoring`.

use server::{db, watchlist::activate_learned_weights};

#[tokio::test]
async fn a_broken_weights_table_never_stops_startup_and_defaults_are_used() {
    let pool = db::connect_memory().await.unwrap();

    // Sin versiones: pesos por defecto.
    ai::scoring::set_learned_multipliers(Some([2.0; 7]));
    assert!(activate_learned_weights(&pool).await.is_none());
    assert_eq!(ai::scoring::learned_multipliers(), None);

    // Fila activa con multiplicadores ilegibles o de otro tamaño: se ignora, pesos por defecto.
    for bad in ["esto no es json", "[1.0, 2.0]"] {
        sqlx::query("DELETE FROM scoring_weights").execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO scoring_weights (version, kind, status, multipliers, created_at) VALUES (1, 'learned', 'active', ?1, 0)")
            .bind(bad)
            .execute(&pool)
            .await
            .unwrap();
        ai::scoring::set_learned_multipliers(Some([2.0; 7]));
        assert!(activate_learned_weights(&pool).await.is_none(), "{bad}");
        assert_eq!(ai::scoring::learned_multipliers(), None, "{bad}");
    }

    // Una versión válida sí se instala.
    sqlx::query("DELETE FROM scoring_weights").execute(&pool).await.unwrap();
    sqlx::query("INSERT INTO scoring_weights (version, kind, status, multipliers, created_at) VALUES (2, 'learned', 'active', '[1.5,1,1,1,1,1,1]', 0)")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(activate_learned_weights(&pool).await.map(|v| v.version), Some(2));
    assert_eq!(ai::scoring::learned_multipliers(), Some([1.5, 1.0, 1.0, 1.0, 1.0, 1.0, 1.0]));

    // La consulta falla (tabla inexistente): se registra y se limpian los pesos, sin panic ni error.
    sqlx::query("DROP TABLE scoring_weights").execute(&pool).await.unwrap();
    assert!(activate_learned_weights(&pool).await.is_none());
    assert_eq!(ai::scoring::learned_multipliers(), None, "no se queda nada a medias");
}
