// API client wrapper
window.Api = {
    async getEvents(page = 1, pageSize = 20) {
        const res = await fetch(`/api/events?page=${page}&page_size=${pageSize}`);
        if (!res.ok) throw new Error(`Events API error: ${res.status}`);
        return res.json();
    },

    async getEvent(id) {
        const res = await fetch(`/api/events/${id}`);
        if (!res.ok) throw new Error(`Event API error: ${res.status}`);
        return res.json();
    },

    async getFactionStats() {
        const res = await fetch('/api/meta/factions');
        if (!res.ok) throw new Error(`Meta API error: ${res.status}`);
        return res.json();
    },

    async getEpochs() {
        const res = await fetch('/api/epochs');
        if (!res.ok) throw new Error(`Epochs API error: ${res.status}`);
        return res.json();
    }
};
