import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import {
  type AwardDeal,
  type FlyingBlueBuyRates,
  type RouteDirection,
  type SearchResult,
  type TrackerConfig,
  ROUTES,
  formatDuration,
  formatMiles,
  getBestDeals,
  loadConfig,
  refreshFlyingBlueBuyRates,
  saveConfig,
  searchBothRoutes,
} from '../services/awardFlights';
import { formatCad, formatCents } from '../services/flyingBluePurchase';

type ViewMode = 'all' | RouteDirection;

function DealCard({ deal, rank }: { deal: AwardDeal; rank: number }) {
  const isTop = rank === 1;

  return (
    <article className={`award-deal-card ${isTop ? 'award-deal-best' : ''}`}>
      <div className="award-deal-rank">{isTop ? '🏆' : `#${rank}`}</div>
      <div className="award-deal-main">
        <div className="award-deal-header">
          <span className="award-deal-route">{deal.route}</span>
          <span className="award-deal-date">{deal.date}</span>
          {deal.direct && <span className="award-badge direct">Direct AF</span>}
          {deal.seats > 0 && (
            <span className="award-badge seats">{deal.seats} place{deal.seats > 1 ? 's' : ''}</span>
          )}
        </div>
        <div className="award-deal-program">{deal.programLabel}</div>
        <div className="award-deal-meta">
          <span>✈ {deal.airlines}</span>
          {deal.flightNumbers && <span>{deal.flightNumbers}</span>}
          {deal.stops != null && <span>{deal.stops === 0 ? 'Sans escale' : `${deal.stops} escale(s)`}</span>}
          {deal.durationMinutes != null && <span>{formatDuration(deal.durationMinutes)}</span>}
        </div>
        <div className="award-cost-breakdown">
          <span>Miles : {formatCad(deal.milesCostCad)}</span>
          <span>Taxes : {formatCad(deal.taxesCad)}</span>
        </div>
      </div>
      <div className="award-deal-price">
        <div className="award-total-label">Total</div>
        <div className="award-total-cad">{formatCad(deal.totalCostCad)}</div>
        <div className="award-miles-sub">{formatMiles(deal.miles)} miles FB</div>
        <a
          className="award-book-btn"
          href={deal.bookingUrl}
          target="_blank"
          rel="noopener noreferrer"
        >
          Réserver →
        </a>
      </div>
    </article>
  );
}

function RouteSection({
  title,
  result,
  loading,
}: {
  title: string;
  result: SearchResult | null;
  loading: boolean;
}) {
  const deals = result?.deals ?? [];
  const best = getBestDeals(deals, 10);

  return (
    <section className="award-route-section">
      <div className="award-route-header">
        <h4>{title}</h4>
        <span className="award-route-count">
          {loading ? '…' : `${deals.length} vol${deals.length !== 1 ? 's' : ''} AF`}
        </span>
      </div>
      {result?.error && <div className="award-error">{result.error}</div>}
      {!loading && !result?.error && best.length === 0 && (
        <div className="award-empty">Aucune place business Air France Flying Blue sur cette période.</div>
      )}
      <div className="award-deals-list">
        {best.map((deal, i) => (
          <DealCard key={deal.id} deal={deal} rank={i + 1} />
        ))}
      </div>
    </section>
  );
}

export function AwardFlightsPlugin() {
  const [config, setConfig] = useState<TrackerConfig>(loadConfig);
  const [showSettings, setShowSettings] = useState(!config.apiKey);
  const [loading, setLoading] = useState(false);
  const [view, setView] = useState<ViewMode>('all');
  const [buyRates, setBuyRates] = useState<FlyingBlueBuyRates | null>(null);
  const [results, setResults] = useState<{
    montrealParis: SearchResult | null;
    parisMontreal: SearchResult | null;
  }>({ montrealParis: null, parisMontreal: null });
  const [lastRefresh, setLastRefresh] = useState<number | null>(null);
  const [countdown, setCountdown] = useState(0);
  const prevBestRef = useRef<Map<string, number>>(new Map());

  const persistConfig = useCallback((next: TrackerConfig) => {
    setConfig(next);
    saveConfig(next);
  }, []);

  const resolveBuyRates = useCallback(async () => {
    const scenario = config.buyScenario === 'auto' ? undefined : config.buyScenario;
    const rates = await refreshFlyingBlueBuyRates(scenario);
    setBuyRates(rates);
    return rates;
  }, [config.buyScenario]);

  const runSearch = useCallback(async (silent = false) => {
    if (!config.apiKey.trim()) {
      setShowSettings(true);
      return;
    }

    if (!silent) setLoading(true);
    try {
      const rates = await resolveBuyRates();
      const data = await searchBothRoutes(config.apiKey, config.dateRangeDays, rates);
      setResults(data);
      setLastRefresh(Date.now());

      const allDeals = [...data.montrealParis.deals, ...data.parisMontreal.deals];
      const alerts: string[] = [];

      for (const deal of allDeals) {
        const key = `${deal.route}-${deal.date}`;
        const prevTotal = prevBestRef.current.get(key);
        const isNewOrBetter = prevTotal == null || deal.totalCostCad < prevTotal;
        const underThreshold = deal.totalCostCad <= config.alertMaxTotalCad;

        if (isNewOrBetter && underThreshold && prevBestRef.current.size > 0) {
          alerts.push(
            `${deal.route} ${deal.date}: ${formatCad(deal.totalCostCad)} (${formatMiles(deal.miles)} mi)`,
          );
        }
        prevBestRef.current.set(key, deal.totalCostCad);
      }

      if (alerts.length > 0 && window.supremacy?.showNotification) {
        await window.supremacy.showNotification(
          '🇫🇷 Meilleure offre Flying Blue AF',
          alerts.slice(0, 3).join('\n'),
        );
      }
    } finally {
      if (!silent) setLoading(false);
    }
  }, [config, resolveBuyRates]);

  useEffect(() => {
    resolveBuyRates();
    if (config.apiKey) runSearch();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (!config.autoRefresh || !config.apiKey) return;

    const intervalMs = config.refreshMinutes * 60 * 1000;
    setCountdown(config.refreshMinutes * 60);

    const tick = setInterval(() => {
      setCountdown((c) => {
        if (c <= 1) {
          runSearch(true);
          return config.refreshMinutes * 60;
        }
        return c - 1;
      });
    }, 1000);

    const refreshTimer = setInterval(() => runSearch(true), intervalMs);

    return () => {
      clearInterval(tick);
      clearInterval(refreshTimer);
    };
  }, [config.autoRefresh, config.refreshMinutes, config.apiKey, runSearch]);

  const globalBest = useMemo(() => {
    const all = [
      ...(results.montrealParis?.deals ?? []),
      ...(results.parisMontreal?.deals ?? []),
    ];
    return getBestDeals(all, 3);
  }, [results]);

  return (
    <div className="plugin-content award-tracker">
      <div className="award-tracker-header">
        <div>
          <h3>🇫🇷 Flying Blue — Air France YUL ↔ PAR</h3>
          <p className="award-subtitle">
            Business class · vols Air France uniquement · coût total = achat miles + taxes
          </p>
        </div>
        <div className="award-header-actions">
          <button
            className="award-icon-btn"
            onClick={() => setShowSettings((s) => !s)}
            title="Paramètres"
          >
            ⚙
          </button>
          <button onClick={() => runSearch()} disabled={loading}>
            {loading ? 'Scan…' : 'Scanner maintenant'}
          </button>
        </div>
      </div>

      {buyRates && (
        <div className="award-buy-rates glass">
          <div className="award-buy-rates-header">
            <span className={`award-promo-badge ${buyRates.promoActive ? 'active' : ''}`}>
              {buyRates.promoActive ? '🔥 PROMO ACTIVE' : 'Tarif standard'}
            </span>
            <span>{buyRates.scenarioLabel}</span>
          </div>
          <div className="award-buy-rates-grid">
            <div>
              <span className="award-rate-label">Prix achat mile</span>
              <strong>{formatCents(buyRates.centsPerMileUsd)} USD</strong>
              <span className="award-rate-sub">({formatCents(buyRates.centsPerMileCad)} CAD)</span>
            </div>
            <div>
              <span className="award-rate-label">Taux USD/CAD</span>
              <strong>{buyRates.usdToCad.toFixed(4)}</strong>
              <span className="award-rate-sub">live Frankfurter</span>
            </div>
            <div>
              <span className="award-rate-label">Classement</span>
              <strong>Par coût total</strong>
              <span className="award-rate-sub">miles + taxes en CAD</span>
            </div>
          </div>
          <p className="award-promo-note">{buyRates.promoNote}</p>
        </div>
      )}

      {showSettings && (
        <div className="award-settings glass">
          <h4>Configuration</h4>
          <p className="award-settings-help">
            Clé API Seats.aero Pro →{' '}
            <a href="https://seats.aero/settings" target="_blank" rel="noopener noreferrer">
              seats.aero/settings
            </a>
            {' · '}
            Acheter miles FB →{' '}
            <a href="https://www.flyingblue.com/en/buy-miles" target="_blank" rel="noopener noreferrer">
              flyingblue.com/buy-miles
            </a>
          </p>
          <label className="award-label">Clé API Seats.aero</label>
          <input
            className="plugin-input"
            type="password"
            value={config.apiKey}
            onChange={(e) => persistConfig({ ...config, apiKey: e.target.value })}
            placeholder="Partner-Authorization key…"
          />
          <div className="award-settings-grid">
            <label>
              <span>Scénario achat miles</span>
              <select
                value={config.buyScenario}
                onChange={(e) =>
                  persistConfig({
                    ...config,
                    buyScenario: e.target.value as TrackerConfig['buyScenario'],
                  })
                }
              >
                <option value="auto">Auto (promo en cours)</option>
                <option value="promo80">Promo +80% bonus</option>
                <option value="promo45">Promo -45%</option>
                <option value="standard">Tarif standard</option>
              </select>
            </label>
            <label>
              <span>Alerte si total ≤ (CAD)</span>
              <input
                type="number"
                min={500}
                max={20000}
                step={100}
                value={config.alertMaxTotalCad}
                onChange={(e) =>
                  persistConfig({ ...config, alertMaxTotalCad: Number(e.target.value) || 2500 })
                }
              />
            </label>
            <label>
              <span>Rafraîchissement (min)</span>
              <input
                type="number"
                min={1}
                max={60}
                value={config.refreshMinutes}
                onChange={(e) =>
                  persistConfig({ ...config, refreshMinutes: Number(e.target.value) || 5 })
                }
              />
            </label>
          </div>
          <label className="toggle-row">
            <span>Surveillance automatique</span>
            <input
              type="checkbox"
              checked={config.autoRefresh}
              onChange={(e) => persistConfig({ ...config, autoRefresh: e.target.checked })}
            />
          </label>
        </div>
      )}

      <div className="award-status-bar">
        <span className={`award-live ${config.autoRefresh ? 'active' : ''}`}>
          {config.autoRefresh ? '● LIVE' : '○ PAUSE'}
        </span>
        {lastRefresh && (
          <span>Dernière mise à jour : {new Date(lastRefresh).toLocaleTimeString('fr-CA')}</span>
        )}
        {config.autoRefresh && config.apiKey && (
          <span>Prochain scan : {Math.floor(countdown / 60)}:{(countdown % 60).toString().padStart(2, '0')}</span>
        )}
      </div>

      {globalBest.length > 0 && (
        <div className="award-global-best glass">
          <h4>🏆 Top 3 — billet le moins cher (Flying Blue AF)</h4>
          <div className="award-top-row">
            {globalBest.map((deal, i) => (
              <div key={deal.id} className="award-top-chip">
                <span className="award-top-rank">#{i + 1}</span>
                <span>{deal.route}</span>
                <strong>{formatCad(deal.totalCostCad)}</strong>
                <span>{deal.date}</span>
                <span>{formatMiles(deal.miles)} mi</span>
              </div>
            ))}
          </div>
        </div>
      )}

      <div className="award-view-tabs">
        <button className={view === 'all' ? 'active' : ''} onClick={() => setView('all')}>
          Les deux sens
        </button>
        <button className={view === 'YUL-PAR' ? 'active' : ''} onClick={() => setView('YUL-PAR')}>
          {ROUTES.montrealParis.label}
        </button>
        <button className={view === 'PAR-YUL' ? 'active' : ''} onClick={() => setView('PAR-YUL')}>
          {ROUTES.parisMontreal.label}
        </button>
      </div>

      <div className="award-results">
        {(view === 'all' || view === 'YUL-PAR') && (
          <RouteSection
            title={ROUTES.montrealParis.label}
            result={results.montrealParis}
            loading={loading}
          />
        )}
        {(view === 'all' || view === 'PAR-YUL') && (
          <RouteSection
            title={ROUTES.parisMontreal.label}
            result={results.parisMontreal}
            loading={loading}
          />
        )}
      </div>

      <div className="award-footer-note">
        Filtre strict : programme Flying Blue + compagnie Air France (AF) uniquement.
        Coût total = miles achetés au tarif promo en vigueur + taxes aéroport.
      </div>
    </div>
  );
}
