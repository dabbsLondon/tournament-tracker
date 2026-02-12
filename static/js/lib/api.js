export function createApi(fetchFn) {
    var _fetch = fetchFn || (typeof globalThis !== 'undefined' ? globalThis.fetch : undefined);

    return {
        async getEvents(page, pageSize, epoch, from, to, hasResults) {
            var params = 'page=' + (page||1) + '&page_size=' + (pageSize||20);
            if (epoch && epoch !== 'current') params += '&epoch=' + encodeURIComponent(epoch);
            if (from) params += '&from=' + from;
            if (to) params += '&to=' + to;
            if (hasResults) params += '&has_results=true';
            var res = await _fetch('/api/events?' + params);
            if (!res.ok) throw new Error('Events API error: ' + res.status);
            return res.json();
        },
        async getEvent(id, epoch) {
            var params = '';
            if (epoch && epoch !== 'current') params = '?epoch=' + encodeURIComponent(epoch);
            var res = await _fetch('/api/events/' + id + params);
            if (!res.ok) throw new Error('Event API error: ' + res.status);
            return res.json();
        },
        async getFactionStats(epoch, from, to) {
            var parts = [];
            if (epoch && epoch !== 'current') parts.push('epoch=' + encodeURIComponent(epoch));
            if (from) parts.push('from=' + from);
            if (to) parts.push('to=' + to);
            var params = parts.length > 0 ? '?' + parts.join('&') : '';
            var res = await _fetch('/api/meta/factions' + params);
            if (!res.ok) throw new Error('Meta API error: ' + res.status);
            return res.json();
        },
        async getEpochs() {
            var res = await _fetch('/api/epochs');
            if (!res.ok) throw new Error('Epochs API error: ' + res.status);
            return res.json();
        },
    };
}

if (typeof window !== 'undefined') {
    window.createApi = createApi;
}
