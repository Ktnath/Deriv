import re

with open('d:/Deriv/bot_v4_trades.log', encoding='utf-8') as f:
    lines = f.read().splitlines()

trades = []
current_trade = {}

for line in lines:
    if 'Trade FSM: IDLE → PRICING' in line:
        current_trade = {'setup': 'Unknown'}
        if 'contract_type=CALL' in line: current_trade['type'] = 'CALL'
        elif 'contract_type=PUT' in line: current_trade['type'] = 'PUT'
        
        idx = lines.index(line)
        for i in range(max(0, idx-5), idx):
            if 'Adaptive Setup Triggered' in lines[i]:
                if 'setup="Bullish_DipBuy"' in lines[i]: current_trade['setup'] = 'Bullish_DipBuy'
                elif 'setup="Bearish_RallySell"' in lines[i]: current_trade['setup'] = 'Bearish_RallySell'
                elif 'setup="Neutral' in lines[i]: current_trade['setup'] = 'Neutral'
                else: current_trade['setup'] = lines[i]
                
    elif 'real_pnl=' in line and current_trade:
        try:
            pnl = float(line.split('real_pnl=')[1].split()[0])
            current_trade['pnl'] = pnl
            if pnl > 0: current_trade['result'] = 'WIN'
            else: current_trade['result'] = 'LOSS'
            trades.append(current_trade)
            current_trade = {}
        except Exception as e:
            print("Err pnl parse:", e)

total_trades = len(trades)
wins = sum(1 for t in trades if t.get('result') == 'WIN')
losses = sum(1 for t in trades if t.get('result') == 'LOSS')
pnl = sum(t.get('pnl', 0) for t in trades)

print(f"Total Trades: {total_trades}")
print(f"Wins: {wins}, Losses: {losses}")
if total_trades > 0:
    print(f"Win Rate: {wins/total_trades*100:.1f}%")
print(f"Net PnL: ${pnl:.2f}")

setup_stats = {}
for t in trades:
    s = t.get('setup', 'Unknown')
    if s not in setup_stats: setup_stats[s] = {'trades': 0, 'wins': 0, 'pnl': 0}
    setup_stats[s]['trades'] += 1
    if t.get('result') == 'WIN': setup_stats[s]['wins'] += 1
    setup_stats[s]['pnl'] += t.get('pnl', 0)

print('\nBreakdown:')
for s, stats in setup_stats.items():
    wr = stats['wins'] / stats['trades'] * 100 if stats['trades'] else 0
    print(f"{s}: {stats['trades']} trades, {stats['wins']} wins ({wr:.1f}%), PnL: ${stats['pnl']:.2f}")

