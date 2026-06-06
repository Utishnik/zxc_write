TODO

@echo off
setlocal

REM === Установка флагов Miri ===
REM -Zmiri-backtrace=full : Полный стек вызовов при обнаружении ошибок (UB)
set "MIRIFLAGS=-Zmiri-backtrace=full"

REM === Установка размера стека для Miri (8 МБ = 8388608 байт) ===
REM Это делается через отдельную переменную окружения, а не через MIRIFLAGS
set "MIRI_STACK_SIZE=8388608"

REM === Установка флага компиляции для условной компиляции ===
set "RUSTFLAGS=--cfg=miri"

REM === Запуск Miri ===
echo [INFO] Запуск cargo +nightly miri run...
cargo +nightly miri run

REM === Очистка переменных окружения ===
set "MIRIFLAGS="
set "MIRI_STACK_SIZE="
set "RUSTFLAGS="

echo [INFO] Выполнение завершено.
pause