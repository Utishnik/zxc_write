//! zxc

#![warn(
    // === RUSTC ВСТРОЕННЫЕ ЛИНТЫ ===
    missing_debug_implementations,
    elided_lifetimes_in_paths,
    explicit_outlives_requirements,
    trivial_casts,
    trivial_numeric_casts,

    // === CLIPPY ЛИНТЫ ===
    // Группы
    // clippy::restriction, // Раскомментируйте, если нужен максимальный уровень (очень много предупреждений)
    clippy::cargo,

    // Явные типы и аннотации
    clippy::let_underscore_untyped,
    clippy::implicit_hasher,
    clippy::implicit_saturating_sub,
    clippy::implicit_clone,
    clippy::default_constructed_unit_structs,

    // Документация и паника
    clippy::missing_const_for_fn,
    clippy::missing_assert_message,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::missing_safety_doc,

    // Упрощение и читаемость
    clippy::let_unit_value,
    clippy::unnecessary_wraps,
    clippy::unnecessary_lazy_evaluations,
    clippy::unseparated_literal_suffix,
    clippy::inconsistent_digit_grouping,
)]
#![allow(
    // Отключаем то, что может раздражать
    clippy::module_name_repetitions,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::redundant_type_annotations, // не требовать убирать явные типы
)]

pub mod find_proccess;
pub mod mem;
