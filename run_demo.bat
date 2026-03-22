@echo off
echo ==============================================
echo ====   LANCEMENT DU BOT DERIV (DEMO)      ====
echo ==============================================
if not exist data mkdir data
cargo run --bin executor
pause
