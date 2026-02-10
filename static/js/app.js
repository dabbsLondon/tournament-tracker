// Main app component

function App() {
    const [factions, setFactions] = React.useState(null);
    const [epochs, setEpochs] = React.useState([]);
    const [activeEpoch, setActiveEpoch] = React.useState('current');
    const [events, setEvents] = React.useState(null);
    const [pagination, setPagination] = React.useState(null);
    const [page, setPage] = React.useState(1);
    const [selectedEvent, setSelectedEvent] = React.useState(null);
    const [loading, setLoading] = React.useState(true);

    // Load initial data
    React.useEffect(() => {
        Promise.all([
            Api.getFactionStats(),
            Api.getEpochs(),
            Api.getEvents(1, 20),
        ]).then(([factionData, epochData, eventData]) => {
            setFactions(factionData.factions);
            setEpochs(epochData.epochs);
            setEvents(eventData.events);
            setPagination(eventData.pagination);
            setLoading(false);
        }).catch(err => {
            console.error('Failed to load data:', err);
            setLoading(false);
        });
    }, []);

    const handlePageChange = (newPage) => {
        setPage(newPage);
        Api.getEvents(newPage, 20).then(data => {
            setEvents(data.events);
            setPagination(data.pagination);
        });
    };

    const handleSelectEvent = (id) => {
        setSelectedEvent(id);
    };

    const handleBack = () => {
        setSelectedEvent(null);
    };

    if (loading) {
        return (
            <div>
                <div className="header">
                    <h1>40K META TRACKER</h1>
                </div>
                <div className="loading" style={{ height: '80vh' }}>
                    <div className="spinner"></div>
                    Loading dashboard...
                </div>
            </div>
        );
    }

    return (
        <div>
            <div className="header">
                <h1>40K META TRACKER</h1>
                <div className="epoch-bar">
                    {epochs.map(epoch => (
                        <span
                            key={epoch.id}
                            className={`epoch-pill ${epoch.is_current ? 'active' : ''}`}
                            onClick={() => setActiveEpoch(epoch.id)}
                        >
                            {epoch.label}
                        </span>
                    ))}
                </div>
            </div>

            <div className="layout">
                <div className="sidebar">
                    <h2>Faction Meta</h2>
                    <FactionChart factions={factions} />
                    <FactionCards factions={factions} />
                </div>

                <div className="main">
                    {selectedEvent ? (
                        <EventDetail
                            eventId={selectedEvent}
                            onBack={handleBack}
                        />
                    ) : (
                        <div>
                            <h2>Recent Tournaments</h2>
                            <EventList
                                events={events}
                                pagination={pagination}
                                onPageChange={handlePageChange}
                                onSelectEvent={handleSelectEvent}
                            />
                        </div>
                    )}
                </div>
            </div>
        </div>
    );
}

const root = ReactDOM.createRoot(document.getElementById('root'));
root.render(<App />);
