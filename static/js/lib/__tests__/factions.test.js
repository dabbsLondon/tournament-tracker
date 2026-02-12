import { describe, it, expect } from 'vitest';
import { FACTION_ICONS, FACTION_ALLEGIANCE_MAP, FACTION_ICON_COLORS } from '../factions.js';

describe('factions', () => {
    describe('FACTION_ICONS', () => {
        it('has entries for all major factions', () => {
            expect(FACTION_ICONS['Space Marines']).toBeDefined();
            expect(FACTION_ICONS['Aeldari']).toBeDefined();
            expect(FACTION_ICONS['Chaos Space Marines']).toBeDefined();
            expect(FACTION_ICONS['Necrons']).toBeDefined();
        });

        it('each entry has abbr and img keys', () => {
            for (const [name, info] of Object.entries(FACTION_ICONS)) {
                expect(info).toHaveProperty('abbr');
                expect(info).toHaveProperty('img');
                expect(typeof info.abbr).toBe('string');
                expect(typeof info.img).toBe('string');
            }
        });

        it('has at least 25 factions', () => {
            expect(Object.keys(FACTION_ICONS).length).toBeGreaterThanOrEqual(25);
        });
    });

    describe('FACTION_ALLEGIANCE_MAP', () => {
        it('maps all factions to valid allegiance values', () => {
            const validAllegiances = ['Imperium', 'Chaos', 'Xenos'];
            for (const [name, allegiance] of Object.entries(FACTION_ALLEGIANCE_MAP)) {
                expect(validAllegiances).toContain(allegiance);
            }
        });

        it('has Imperium, Chaos, and Xenos factions', () => {
            const allegiances = new Set(Object.values(FACTION_ALLEGIANCE_MAP));
            expect(allegiances.has('Imperium')).toBe(true);
            expect(allegiances.has('Chaos')).toBe(true);
            expect(allegiances.has('Xenos')).toBe(true);
        });
    });

    describe('FACTION_ICON_COLORS', () => {
        it('has Imperium, Chaos, Xenos, and Unknown keys', () => {
            expect(FACTION_ICON_COLORS).toHaveProperty('Imperium');
            expect(FACTION_ICON_COLORS).toHaveProperty('Chaos');
            expect(FACTION_ICON_COLORS).toHaveProperty('Xenos');
            expect(FACTION_ICON_COLORS).toHaveProperty('Unknown');
        });

        it('all values are hex color strings', () => {
            for (const color of Object.values(FACTION_ICON_COLORS)) {
                expect(color).toMatch(/^#[0-9a-f]{6}$/i);
            }
        });
    });

    describe('cross-consistency', () => {
        it('every faction in ALLEGIANCE_MAP exists in ICONS', () => {
            for (const name of Object.keys(FACTION_ALLEGIANCE_MAP)) {
                expect(FACTION_ICONS[name]).toBeDefined();
            }
        });
    });
});
