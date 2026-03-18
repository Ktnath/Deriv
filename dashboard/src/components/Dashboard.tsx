import React, { useEffect, useState } from 'react';

interface TickData {
    type: string;
    price: number;
    time: number;
    signal: string;
    edge: string;
    pnl: string;
}

const Dashboard: React.FC = () => {
    const [ticks, setTicks] = useState<TickData[]>([]);
    const [connected, setConnected] = useState(false);

    useEffect(() => {
        const ws = new WebSocket('ws://127.0.0.1:3000/ws');

        ws.onopen = () => setConnected(true);
        ws.onclose = () => setConnected(false);

        ws.onmessage = (event) => {
            try {
                const data = JSON.parse(event.data);
                if (data.type === 'tick') {
                    setTicks((prev) => [...prev, data].slice(-50)); // Keep last 50 ticks
                }
            } catch (err) {
                console.error('Error parsing WS message:', err);
            }
        };

        return () => ws.close();
    }, []);

    const latest = ticks[ticks.length - 1];

    return (
        <div className="min-h-screen bg-neutral-950 text-neutral-50 p-6 font-sans">
            <header className="mb-8 flex justify-between items-center bg-neutral-900 p-6 rounded-2xl shadow-xl border border-neutral-800">
                <div>
                    <h1 className="text-3xl font-bold tracking-tight bg-gradient-to-r from-emerald-400 to-cyan-400 bg-clip-text text-transparent">
                        Deriv Bot Dashboard
                    </h1>
                    <p className="text-neutral-400 mt-1">Live Trading Telemetry</p>
                </div>
                <div className="flex items-center space-x-3">
                    <div className={`w-3 h-3 rounded-full ${connected ? 'bg-emerald-500 animate-pulse' : 'bg-red-500'}`} />
                    <span className="font-medium text-neutral-300 bg-neutral-800/50 px-3 py-1 rounded-full border border-neutral-700">
                        {connected ? 'ws://127.0.0.1:3000' : 'Disconnected'}
                    </span>
                </div>
            </header>

            <div className="grid grid-cols-1 md:grid-cols-3 gap-6 mb-8">
                <div className="bg-neutral-900 p-6 rounded-2xl shadow-lg border border-neutral-800 flex flex-col justify-center">
                    <h2 className="text-neutral-400 text-sm font-semibold uppercase tracking-wider mb-2">Current Price</h2>
                    <div className="text-4xl font-bold text-white tracking-tight">
                        {latest ? latest.price.toFixed(4) : '---'}
                    </div>
                </div>

                <div className="bg-neutral-900 p-6 rounded-2xl shadow-lg border border-neutral-800 flex flex-col justify-center">
                    <h2 className="text-neutral-400 text-sm font-semibold uppercase tracking-wider mb-2">Realized PnL</h2>
                    <div className={`text-4xl font-bold tracking-tight ${latest && parseFloat(latest.pnl) >= 0 ? 'text-emerald-400' : 'text-red-400'}`}>
                        ${latest ? latest.pnl : '0.00'}
                    </div>
                </div>

                <div className="bg-neutral-900 p-6 rounded-2xl shadow-lg border border-neutral-800 flex flex-col justify-center">
                    <h2 className="text-neutral-400 text-sm font-semibold uppercase tracking-wider mb-2">Strategy Edge</h2>
                    <div className="text-4xl font-bold text-cyan-400 tracking-tight">
                        {latest ? latest.edge : '---'}
                    </div>
                </div>
            </div>

            <div className="bg-neutral-900 rounded-2xl shadow-xl border border-neutral-800 overflow-hidden">
                <div className="p-6 border-b border-neutral-800 bg-neutral-900/50">
                    <h2 className="text-xl font-bold text-neutral-200">Recent Signals</h2>
                </div>
                <div className="p-6 overflow-x-auto">
                    <table className="w-full text-left border-collapse">
                        <thead>
                            <tr className="text-neutral-400 text-sm uppercase tracking-wider border-b border-neutral-800">
                                <th className="pb-4 font-semibold">Time</th>
                                <th className="pb-4 font-semibold">Price</th>
                                <th className="pb-4 font-semibold">Edge</th>
                                <th className="pb-4 font-semibold">Ensemble Signal</th>
                            </tr>
                        </thead>
                        <tbody className="text-sm">
                            {[...ticks].reverse().slice(0, 10).map((tick, i) => (
                                <tr key={i} className="border-b border-neutral-800/50 hover:bg-neutral-800/30 transition-colors">
                                    <td className="py-4 text-neutral-300 font-mono">
                                        {new Date(tick.time * 1000).toLocaleTimeString()}
                                    </td>
                                    <td className="py-4 font-medium text-white">{tick.price.toFixed(4)}</td>
                                    <td className="py-4 text-cyan-400 font-mono">{tick.edge}</td>
                                    <td className="py-4">
                                        <span className={`px-2 py-1 rounded text-xs font-bold ${tick.signal.includes('Call') ? 'bg-emerald-500/20 text-emerald-400 border border-emerald-500/30' :
                                                tick.signal.includes('Put') ? 'bg-red-500/20 text-red-400 border border-red-500/30' :
                                                    'bg-neutral-800 text-neutral-400 border border-neutral-700'
                                            }`}>
                                            {tick.signal}
                                        </span>
                                    </td>
                                </tr>
                            ))}
                            {ticks.length === 0 && (
                                <tr>
                                    <td colSpan={4} className="py-8 text-center text-neutral-500 italic">
                                        Waiting for tick data from Axum server...
                                    </td>
                                </tr>
                            )}
                        </tbody>
                    </table>
                </div>
            </div>
        </div>
    );
};

export default Dashboard;
