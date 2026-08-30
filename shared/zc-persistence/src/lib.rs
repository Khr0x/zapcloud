//! zc-persistence — abstracción de almacenamiento de metadata.
//!
//! SQLite por defecto (single-node), PostgreSQL opcional para alta frecuencia
//! y multi-node (§57, §64). Expone el trait de repositorio y las migraciones;
//! el modelo de datos inicial está en el RFC de Lambda §58.
//!
//! v0.1: stub. Primer uso real desde `function-manager` (tablas functions,
//! artifacts). Kernel: no depende de ningún servicio.
