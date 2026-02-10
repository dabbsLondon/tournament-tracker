// Faction meta chart component
const { useState, useEffect, useRef } = React;

const TIER_COLORS = {
    S: '#ff4444',
    A: '#ff8844',
    B: '#d4af37',
    C: '#44aa88',
    D: '#6688aa',
};

window.FactionChart = function FactionChart({ factions }) {
    const chartRef = useRef(null);
    const chartInstance = useRef(null);

    useEffect(() => {
        if (!chartRef.current || !factions || factions.length === 0) return;

        // Take top 15 for chart readability
        const top = factions.slice(0, 15);

        if (chartInstance.current) {
            chartInstance.current.destroy();
        }

        const ctx = chartRef.current.getContext('2d');
        chartInstance.current = new Chart(ctx, {
            type: 'bar',
            data: {
                labels: top.map(f => f.faction),
                datasets: [{
                    label: 'Meta Share %',
                    data: top.map(f => f.meta_share),
                    backgroundColor: top.map(f => TIER_COLORS[f.tier] || '#6688aa'),
                    borderColor: 'transparent',
                    borderRadius: 4,
                    barThickness: 18,
                }]
            },
            options: {
                indexAxis: 'y',
                responsive: true,
                maintainAspectRatio: false,
                plugins: {
                    legend: { display: false },
                    tooltip: {
                        backgroundColor: '#1a1a2e',
                        titleColor: '#d4af37',
                        bodyColor: '#e0e0e0',
                        borderColor: '#2a2a4a',
                        borderWidth: 1,
                        callbacks: {
                            afterLabel: function(context) {
                                const f = top[context.dataIndex];
                                return `Top 4: ${f.top4_count} (${f.top4_rate}%)\nWins: ${f.first_place_count}\nTier: ${f.tier}`;
                            }
                        }
                    }
                },
                scales: {
                    x: {
                        grid: { color: '#2a2a4a' },
                        ticks: { color: '#888', font: { size: 11 } },
                        title: {
                            display: true,
                            text: 'Meta Share %',
                            color: '#888',
                            font: { size: 11 }
                        }
                    },
                    y: {
                        grid: { display: false },
                        ticks: { color: '#e0e0e0', font: { size: 12 } }
                    }
                }
            }
        });

        return () => {
            if (chartInstance.current) {
                chartInstance.current.destroy();
            }
        };
    }, [factions]);

    const height = Math.max(300, (factions ? Math.min(factions.length, 15) : 10) * 30 + 40);

    return (
        <div className="chart-container" style={{ height: height + 'px' }}>
            <canvas ref={chartRef}></canvas>
        </div>
    );
};

window.FactionCards = function FactionCards({ factions }) {
    if (!factions || factions.length === 0) {
        return <div className="loading">No faction data</div>;
    }

    return (
        <div className="faction-cards">
            {factions.map(f => (
                <div key={f.faction} className="faction-card">
                    <div className="faction-card-header">
                        <span className="faction-name">{f.faction}</span>
                        <span className={`tier-badge tier-${f.tier}`}>{f.tier}</span>
                    </div>
                    <div className="faction-stats">
                        <span><span className="faction-stat-value">{f.count}</span> players</span>
                        <span><span className="faction-stat-value">{f.meta_share}%</span> meta</span>
                        <span><span className="faction-stat-value">{f.top4_count}</span> top-4</span>
                        <span><span className="faction-stat-value">{f.first_place_count}</span> wins</span>
                    </div>
                    {f.top_detachments && f.top_detachments.length > 0 && (
                        <div className="faction-detachments">
                            {f.top_detachments.map(d => d.name).join(' · ')}
                        </div>
                    )}
                </div>
            ))}
        </div>
    );
};
