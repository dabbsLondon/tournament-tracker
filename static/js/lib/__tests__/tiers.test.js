import { describe, it, expect } from 'vitest';
import { TIER_COLORS, TIER_LABELS, TIER_CUTS, assignTiers, tierColor, tierLabel } from '../tiers.js';

describe('tiers', () => {
    describe('constants', () => {
        it('has 5 tier colors', () => {
            expect(TIER_COLORS).toHaveLength(5);
        });

        it('has 5 tier labels', () => {
            expect(TIER_LABELS).toEqual(['S', 'A', 'B', 'C', 'D']);
        });

        it('has 4 tier cuts', () => {
            expect(TIER_CUTS).toHaveLength(4);
            expect(TIER_CUTS[0]).toBeLessThan(TIER_CUTS[1]);
        });
    });

    describe('assignTiers', () => {
        it('returns empty tiers for 0 factions', () => {
            const tiers = assignTiers([], f => f.win_rate);
            expect(Object.keys(tiers)).toHaveLength(0);
        });

        it('assigns single faction to S tier', () => {
            const factions = [{ win_rate: 0.55, count: 10 }];
            const tiers = assignTiers(factions, f => f.win_rate);
            expect(tiers[0]).toBe(0); // S tier
        });

        it('assigns factions with count < 3 to D tier', () => {
            const factions = [
                { win_rate: 0.60, count: 2 },
                { win_rate: 0.50, count: 10 },
            ];
            const tiers = assignTiers(factions, f => f.win_rate);
            expect(tiers[0]).toBe(4); // D tier (count < 3)
            expect(tiers[1]).toBe(0); // S tier (only qualifying faction)
        });

        it('distributes 5 qualifying factions across tiers', () => {
            const factions = [
                { win_rate: 0.60, count: 10 },
                { win_rate: 0.55, count: 10 },
                { win_rate: 0.50, count: 10 },
                { win_rate: 0.45, count: 10 },
                { win_rate: 0.40, count: 10 },
            ];
            const tiers = assignTiers(factions, f => f.win_rate);
            expect(tiers[0]).toBe(0); // Best = S
            expect(tiers[4]).toBe(4); // Worst = D
        });

        it('handles all same value', () => {
            const factions = Array.from({length: 5}, () => ({ win_rate: 0.50, count: 10 }));
            const tiers = assignTiers(factions, f => f.win_rate);
            // All should get assigned tiers (first gets S since percentile 0)
            for (let i = 0; i < 5; i++) {
                expect(tiers[i]).toBeDefined();
            }
        });

        it('handles 20 factions with percentile distribution', () => {
            const factions = Array.from({length: 20}, (_, i) => ({
                win_rate: 0.60 - i * 0.01,
                count: 10,
            }));
            const tiers = assignTiers(factions, f => f.win_rate);
            // First few should be S tier
            expect(tiers[0]).toBe(0);
            // Last few should be D tier
            expect(tiers[19]).toBe(4);
        });
    });

    describe('tierColor', () => {
        it('returns correct color for valid index', () => {
            expect(tierColor(0)).toBe('#e05555');
            expect(tierColor(4)).toBe('#5a5c78');
        });

        it('returns D-tier color for invalid index', () => {
            expect(tierColor(99)).toBe(TIER_COLORS[4]);
            expect(tierColor(-1)).toBe(TIER_COLORS[4]);
        });
    });

    describe('tierLabel', () => {
        it('returns correct label for valid index', () => {
            expect(tierLabel(0)).toBe('S');
            expect(tierLabel(4)).toBe('D');
        });

        it('returns D for invalid index', () => {
            expect(tierLabel(99)).toBe('D');
            expect(tierLabel(-1)).toBe('D');
        });
    });
});
