import { describe, it, expect, vi, beforeEach } from 'vitest';
import { createApi } from '../api.js';

describe('api', () => {
    let mockFetch;
    let api;

    beforeEach(() => {
        mockFetch = vi.fn().mockResolvedValue({
            ok: true,
            json: () => Promise.resolve({ data: 'test' }),
        });
        api = createApi(mockFetch);
    });

    describe('getEvents', () => {
        it('constructs correct URL with defaults', async () => {
            await api.getEvents();
            expect(mockFetch).toHaveBeenCalledWith('/api/events?page=1&page_size=20');
        });

        it('passes page and pageSize params', async () => {
            await api.getEvents(2, 50);
            expect(mockFetch).toHaveBeenCalledWith('/api/events?page=2&page_size=50');
        });

        it('adds epoch param when not current', async () => {
            await api.getEvents(1, 20, 'epoch-001');
            const url = mockFetch.mock.calls[0][0];
            expect(url).toContain('epoch=epoch-001');
        });

        it('skips epoch param when current', async () => {
            await api.getEvents(1, 20, 'current');
            const url = mockFetch.mock.calls[0][0];
            expect(url).not.toContain('epoch=');
        });

        it('adds from and to params', async () => {
            await api.getEvents(1, 20, null, '2025-01-01', '2025-06-30');
            const url = mockFetch.mock.calls[0][0];
            expect(url).toContain('from=2025-01-01');
            expect(url).toContain('to=2025-06-30');
        });
    });

    describe('getEvent', () => {
        it('constructs correct URL', async () => {
            await api.getEvent('abc123');
            expect(mockFetch).toHaveBeenCalledWith('/api/events/abc123');
        });

        it('adds epoch param when not current', async () => {
            await api.getEvent('abc123', 'epoch-001');
            expect(mockFetch).toHaveBeenCalledWith('/api/events/abc123?epoch=epoch-001');
        });
    });

    describe('getEpochs', () => {
        it('calls correct endpoint', async () => {
            await api.getEpochs();
            expect(mockFetch).toHaveBeenCalledWith('/api/epochs');
        });
    });

    describe('error handling', () => {
        it('throws on non-ok response for getEvents', async () => {
            mockFetch.mockResolvedValueOnce({ ok: false, status: 500 });
            await expect(api.getEvents()).rejects.toThrow('Events API error: 500');
        });

        it('throws on non-ok response for getEvent', async () => {
            mockFetch.mockResolvedValueOnce({ ok: false, status: 404 });
            await expect(api.getEvent('bad-id')).rejects.toThrow('Event API error: 404');
        });

        it('throws on non-ok response for getEpochs', async () => {
            mockFetch.mockResolvedValueOnce({ ok: false, status: 503 });
            await expect(api.getEpochs()).rejects.toThrow('Epochs API error: 503');
        });
    });
});
