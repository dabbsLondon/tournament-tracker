// Event list and detail components

window.EventList = function EventList({ events, pagination, onPageChange, onSelectEvent }) {
    if (!events) {
        return <div className="loading"><div className="spinner"></div>Loading events...</div>;
    }

    if (events.length === 0) {
        return <div className="loading">No events found</div>;
    }

    return (
        <div>
            <div className="event-list">
                {events.map(event => (
                    <div
                        key={event.id}
                        className="event-card"
                        onClick={() => onSelectEvent(event.id)}
                    >
                        <div className="event-card-header">
                            <span className="event-name">{event.name}</span>
                            {event.player_count && (
                                <span className="event-players">{event.player_count} players</span>
                            )}
                        </div>
                        <div className="event-meta">
                            {event.date}
                            {event.location && ` · ${event.location}`}
                            {event.round_count && ` · ${event.round_count} rounds`}
                        </div>
                        {event.winner && (
                            <div className="event-winner">
                                <span className="event-winner-label">Winner: </span>
                                {event.winner.player_name} ({event.winner.faction}
                                {event.winner.detachment && ` — ${event.winner.detachment}`})
                            </div>
                        )}
                    </div>
                ))}
            </div>

            {pagination && pagination.total_pages > 1 && (
                <div className="pagination">
                    <button
                        disabled={!pagination.has_prev}
                        onClick={() => onPageChange(pagination.page - 1)}
                    >
                        &larr; Prev
                    </button>
                    <span className="pagination-info">
                        Page {pagination.page} of {pagination.total_pages}
                    </span>
                    <button
                        disabled={!pagination.has_next}
                        onClick={() => onPageChange(pagination.page + 1)}
                    >
                        Next &rarr;
                    </button>
                </div>
            )}
        </div>
    );
};

window.EventDetail = function EventDetail({ eventId, onBack }) {
    const [event, setEvent] = React.useState(null);
    const [loading, setLoading] = React.useState(true);
    const [expandedLists, setExpandedLists] = React.useState({});

    React.useEffect(() => {
        setLoading(true);
        Api.getEvent(eventId).then(data => {
            setEvent(data);
            setLoading(false);
        }).catch(err => {
            console.error(err);
            setLoading(false);
        });
    }, [eventId]);

    const toggleList = (id) => {
        setExpandedLists(prev => ({ ...prev, [id]: !prev[id] }));
    };

    if (loading) {
        return <div className="loading"><div className="spinner"></div>Loading event...</div>;
    }

    if (!event) {
        return <div className="loading">Event not found</div>;
    }

    return (
        <div className="detail-view">
            <button className="back-btn" onClick={onBack}>
                &larr; Back to Events
            </button>

            <div className="detail-header">
                <h2>{event.name}</h2>
                <div className="event-meta">
                    {event.date}
                    {event.location && ` · ${event.location}`}
                    {event.player_count && ` · ${event.player_count} players`}
                    {event.round_count && ` · ${event.round_count} rounds`}
                </div>
            </div>

            {event.placements && event.placements.length > 0 && (
                <div>
                    <div className="section-title">Placements</div>
                    <table className="placements-table">
                        <thead>
                            <tr>
                                <th>Rank</th>
                                <th>Player</th>
                                <th>Faction</th>
                                <th>Detachment</th>
                                <th>Record</th>
                            </tr>
                        </thead>
                        <tbody>
                            {event.placements.map((p, i) => (
                                <tr key={i}>
                                    <td className={`rank-cell rank-${p.rank <= 3 ? p.rank : ''}`}>
                                        #{p.rank}
                                    </td>
                                    <td style={{ color: '#fff' }}>{p.player_name}</td>
                                    <td>{p.faction}</td>
                                    <td style={{ color: '#888' }}>{p.detachment || '—'}</td>
                                    <td style={{ color: '#888' }}>
                                        {p.record
                                            ? `${p.record.wins}-${p.record.losses}-${p.record.draws}`
                                            : '—'}
                                    </td>
                                </tr>
                            ))}
                        </tbody>
                    </table>
                </div>
            )}

            {event.army_lists && event.army_lists.length > 0 && (
                <div className="army-lists-section">
                    <div className="section-title">Army Lists</div>
                    {event.army_lists.map((list, i) => (
                        <div key={list.id} className="army-list-item">
                            <button
                                className="army-list-toggle"
                                onClick={() => toggleList(list.id)}
                            >
                                <span>
                                    #{i + 1} — {list.faction}
                                    {list.detachment && ` (${list.detachment})`}
                                    {list.total_points > 0 && ` — ${list.total_points}pts`}
                                </span>
                                <span className={`chevron ${expandedLists[list.id] ? 'open' : ''}`}>
                                    &#9654;
                                </span>
                            </button>
                            {expandedLists[list.id] && (
                                <div className="army-list-content">
                                    {list.raw_text}
                                </div>
                            )}
                        </div>
                    ))}
                </div>
            )}
        </div>
    );
};
