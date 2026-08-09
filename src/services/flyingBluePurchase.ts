/** Tarifs achat miles Flying Blue — mis à jour via FX live + promos connues */

export type BuyScenario = 'promo80' | 'promo45' | 'standard';

export interface FlyingBlueBuyRates {
  scenario: BuyScenario;
  scenarioLabel: string;
  /** Coût effectif par mile en USD (centimes) */
  centsPerMileUsd: number;
  /** Taux de change USD → CAD */
  usdToCad: number;
  /** Coût effectif par mile en CAD (centimes) */
  centsPerMileCad: number;
  promoActive: boolean;
  promoNote: string;
  fetchedAt: number;
}

/** Promo Flying Blue connue : +80% bonus jusqu'au 28 juil. 2026 (achat ≥50k miles) */
const PROMO_80_END = new Date('2026-07-28T23:59:59Z');
/** Promo alternative : -45% sur le prix d'achat */
const PROMO_45_END = new Date('2026-11-05T23:59:59Z');

const FLYING_BLUE_RATES: Record<BuyScenario, { centsPerMileUsd: number; label: string; note: string }> = {
  promo80: {
    centsPerMileUsd: 1.69,
    label: 'Promo +80% bonus',
    note: 'Achat ≥50 000 miles — coût effectif ~1,69 ¢ USD/mile',
  },
  promo45: {
    centsPerMileUsd: 1.68,
    label: 'Promo -45%',
    note: 'Réduction directe sur le prix d\'achat — ~1,68 ¢ USD/mile',
  },
  standard: {
    centsPerMileUsd: 3.05,
    label: 'Tarif standard',
    note: 'Sans promo — ~3,05 ¢ USD/mile (éviter sauf urgence)',
  },
};

function detectActiveScenario(): BuyScenario {
  const now = new Date();
  if (now <= PROMO_80_END) return 'promo80';
  if (now <= PROMO_45_END) return 'promo45';
  return 'standard';
}

async function fetchUsdToCad(): Promise<number> {
  if (!window.supremacy?.httpFetch) return 1.38;

  try {
    const res = await window.supremacy.httpFetch(
      'https://api.frankfurter.app/latest?from=USD&to=CAD',
    );
    if (!res.ok) return 1.38;
    const data = JSON.parse(res.body) as { rates?: { CAD?: number } };
    return data.rates?.CAD ?? 1.38;
  } catch {
    return 1.38;
  }
}

export async function refreshFlyingBlueBuyRates(
  forcedScenario?: BuyScenario,
): Promise<FlyingBlueBuyRates> {
  const scenario = forcedScenario ?? detectActiveScenario();
  const rate = FLYING_BLUE_RATES[scenario];
  const usdToCad = await fetchUsdToCad();
  const centsPerMileCad = rate.centsPerMileUsd * usdToCad;

  return {
    scenario,
    scenarioLabel: rate.label,
    centsPerMileUsd: rate.centsPerMileUsd,
    usdToCad,
    centsPerMileCad,
    promoActive: scenario !== 'standard',
    promoNote: rate.note,
    fetchedAt: Date.now(),
  };
}

export interface DealCostBreakdown {
  miles: number;
  milesCostCad: number;
  taxesCad: number;
  totalCostCad: number;
  centsPerMileCad: number;
  buyScenario: BuyScenario;
}

function convertTaxesToCad(amount: number, currency: string | null, usdToCad: number): number {
  const c = (currency ?? 'EUR').toUpperCase();
  if (c === 'CAD') return amount;
  if (c === 'USD') return amount * usdToCad;
  if (c === 'EUR') return amount * usdToCad * 1.08; // EUR→USD approx
  return amount * usdToCad;
}

export function calculateDealCost(
  miles: number,
  taxes: number | null,
  taxesCurrency: string | null,
  buyRates: FlyingBlueBuyRates,
): DealCostBreakdown {
  const milesCostCad = (miles * buyRates.centsPerMileCad) / 100;
  const taxesCad = taxes != null ? convertTaxesToCad(taxes, taxesCurrency, buyRates.usdToCad) : 0;
  const totalCostCad = milesCostCad + taxesCad;

  return {
    miles,
    milesCostCad,
    taxesCad,
    totalCostCad,
    centsPerMileCad: buyRates.centsPerMileCad,
    buyScenario: buyRates.scenario,
  };
}

export function formatCad(amount: number): string {
  return amount.toLocaleString('fr-CA', { style: 'currency', currency: 'CAD', maximumFractionDigits: 0 });
}

export function formatCents(cents: number): string {
  return `${cents.toFixed(2)} ¢`;
}
