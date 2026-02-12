export var TIER_COLORS = ['#e05555', '#e0884a', '#c9a84c', '#6a8da6', '#5a5c78'];
export var TIER_LABELS = ['S', 'A', 'B', 'C', 'D'];
export var TIER_CUTS = [0.15, 0.40, 0.70, 0.90];

export function assignTiers(factions, getValue) {
    var items = factions.map(function(f, i) {
        return { idx: i, val: getValue(f), count: f.count };
    });
    var qualifying = items.filter(function(it) { return it.count >= 3; });
    qualifying.sort(function(a, b) { return b.val - a.val; });
    var n = qualifying.length;
    var tiers = {};
    for (var r = 0; r < n; r++) {
        var pct = n > 1 ? r / (n - 1) : 0;
        var tier;
        if (pct <= TIER_CUTS[0]) tier = 0;
        else if (pct <= TIER_CUTS[1]) tier = 1;
        else if (pct <= TIER_CUTS[2]) tier = 2;
        else if (pct <= TIER_CUTS[3]) tier = 3;
        else tier = 4;
        tiers[qualifying[r].idx] = tier;
    }
    for (var j = 0; j < items.length; j++) {
        if (tiers[j] === undefined) tiers[j] = 4;
    }
    return tiers;
}

export function tierColor(tierIdx) { return TIER_COLORS[tierIdx] || TIER_COLORS[4]; }
export function tierLabel(tierIdx) { return TIER_LABELS[tierIdx] || 'D'; }

if (typeof window !== 'undefined') {
    window.TIER_COLORS = TIER_COLORS;
    window.TIER_LABELS = TIER_LABELS;
    window.TIER_CUTS = TIER_CUTS;
    window.assignTiers = assignTiers;
    window.tierColor = tierColor;
    window.tierLabel = tierLabel;
}
