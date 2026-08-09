export type RouteDirection = 'YUL-PAR' | 'PAR-YUL';

export const FLYING_BLUE_SOURCE = 'flyingblue';
export const AIR_FRANCE_CARRIER = 'AF';
export const PROGRAM_LABEL = 'Flying Blue — Air France';

export interface AwardDeal {
  id: string;
  route: RouteDirection;
  date: string;
  program: string;
  programLabel: string;
  miles: number;
  taxes: number | null;
  taxesCurrency: string | null;
  seats: number;
  direct: boolean;
  airlines: string;
  stops: number | null;
  durationMinutes: number | null;
  flightNumbers: string | null;
  carriers: string | null;
  updatedAt: string | null;
  bookingUrl: string;
  /** Coût total CAD = achat miles + taxes */
  milesCostCad: number;
  taxesCad: number;
  totalCostCad: number;
}

export interface SearchResult {
  deals: AwardDeal[];
  searchedAt: number;
  error?: string;
}

export interface TrackerConfig {
  apiKey: string;
  autoRefresh: boolean;
  refreshMinutes: number;
  alertMaxTotalCad: number;
  dateRangeDays: number;
  buyScenario: 'auto' | 'promo80' | 'promo45' | 'standard';
}

export const DEFAULT_CONFIG: TrackerConfig = {
  apiKey: '',
  autoRefresh: true,
  refreshMinutes: 5,
  alertMaxTotalCad: 2500,
  dateRangeDays: 90,
  buyScenario: 'auto',
};

export const ROUTES = {
  montrealParis: {
    origin: 'YUL',
    destination: 'PAR',
    direction: 'YUL-PAR' as RouteDirection,
    label: 'Montréal → Paris',
  },
  parisMontreal: {
    origin: 'PAR',
    destination: 'YUL',
    direction: 'PAR-YUL' as RouteDirection,
    label: 'Paris → Montréal',
  },
};

import {
  type FlyingBlueBuyRates,
  calculateDealCost,
  refreshFlyingBlueBuyRates,
} from './flyingBluePurchase';

const API_BASE = 'https://seats.aero/partnerapi';

interface SeatsAvailability {
  ID?: string;
  Date?: string;
  Source?: string;
  UpdatedAt?: string;
  JAvailable?: boolean;
  JMileageCost?: string | number | null;
  JRemainingSeats?: number | null;
  JDirect?: boolean;
  JAirlines?: string;
  Trips?: SeatsTrip[];
}

interface SeatsTrip {
  MileageCost?: number;
  TotalTaxes?: number;
  TaxesCurrency?: string;
  RemainingSeats?: number;
  Stops?: number;
  TotalDuration?: number;
  FlightNumbers?: string;
  Carriers?: string;
  Cabin?: string;
}

function formatDate(d: Date): string {
  return d.toISOString().slice(0, 10);
}

function addDays(d: Date, days: number): Date {
  const next = new Date(d);
  next.setDate(next.getDate() + days);
  return next;
}

function parseMiles(value: string | number | null | undefined): number | null {
  if (value == null || value === '') return null;
  const n = typeof value === 'number' ? value : parseInt(String(value).replace(/,/g, ''), 10);
  return Number.isFinite(n) && n > 0 ? n : null;
}

function isAirFranceOperated(airlines: string, carriers: string | null, flightNumbers: string | null): boolean {
  const hay = `${airlines},${carriers ?? ''},${flightNumbers ?? ''}`.toUpperCase();
  return hay.includes('AF') || hay.includes('AIR FRANCE');
}

function dealFromAvailability(
  item: SeatsAvailability,
  route: RouteDirection,
  buyRates: FlyingBlueBuyRates,
): AwardDeal | null {
  const miles = parseMiles(item.JMileageCost);
  if (!item.JAvailable || miles == null) return null;
  if (item.Source?.toLowerCase() !== FLYING_BLUE_SOURCE) return null;

  const bestTrip = item.Trips
    ?.filter((t) => !t.Cabin || t.Cabin === 'business')
    .sort((a, b) => (a.MileageCost ?? miles) - (b.MileageCost ?? miles))[0];

  const airlines = item.JAirlines ?? bestTrip?.Carriers ?? '—';
  const carriers = bestTrip?.Carriers ?? item.JAirlines ?? null;
  const flightNumbers = bestTrip?.FlightNumbers ?? null;

  if (!isAirFranceOperated(airlines, carriers, flightNumbers)) return null;

  const id = item.ID ?? `${route}-${item.Date}-${item.Source}`;
  const tripMiles = bestTrip?.MileageCost ?? miles;
  const taxes = bestTrip?.TotalTaxes ?? null;
  const taxesCurrency = bestTrip?.TaxesCurrency ?? null;

  const cost = calculateDealCost(tripMiles, taxes, taxesCurrency, buyRates);

  return {
    id,
    route,
    date: item.Date ?? '',
    program: FLYING_BLUE_SOURCE,
    programLabel: PROGRAM_LABEL,
    miles: tripMiles,
    taxes,
    taxesCurrency,
    seats: bestTrip?.RemainingSeats ?? item.JRemainingSeats ?? 0,
    direct: item.JDirect ?? (bestTrip?.Stops === 0),
    airlines,
    stops: bestTrip?.Stops ?? null,
    durationMinutes: bestTrip?.TotalDuration ?? null,
    flightNumbers,
    carriers,
    updatedAt: item.UpdatedAt ?? null,
    bookingUrl: item.ID ? `https://seats.aero/i/${item.ID}` : 'https://seats.aero',
    milesCostCad: cost.milesCostCad,
    taxesCad: cost.taxesCad,
    totalCostCad: cost.totalCostCad,
  };
}

async function fetchSeatsAero(
  apiKey: string,
  params: Record<string, string>,
): Promise<{ data?: SeatsAvailability[]; error?: string }> {
  if (!window.supremacy?.httpFetch) {
    return { error: 'API Supremacy indisponible (lance via Electron)' };
  }

  const query = new URLSearchParams(params).toString();
  const url = `${API_BASE}/search?${query}`;

  const res = await window.supremacy.httpFetch(url, {
    headers: { 'Partner-Authorization': apiKey },
  });

  if (!res.ok) {
    if (res.status === 401 || res.status === 403) {
      return { error: 'Clé API invalide ou accès refusé. Vérifie ton abonnement Pro Seats.aero.' };
    }
    return { error: res.error ?? `Erreur API (${res.status})` };
  }

  try {
    const parsed = JSON.parse(res.body) as { data?: SeatsAvailability[] } | SeatsAvailability[];
    const data = Array.isArray(parsed) ? parsed : parsed.data ?? [];
    return { data };
  } catch {
    return { error: 'Réponse API invalide' };
  }
}

export async function searchRoute(
  apiKey: string,
  origin: string,
  destination: string,
  direction: RouteDirection,
  dateRangeDays: number,
  buyRates: FlyingBlueBuyRates,
): Promise<SearchResult> {
  if (!apiKey.trim()) {
    return { deals: [], searchedAt: Date.now(), error: 'Clé API Seats.aero requise' };
  }

  const start = formatDate(new Date());
  const end = formatDate(addDays(new Date(), dateRangeDays));

  const { data, error } = await fetchSeatsAero(apiKey, {
    origin_airport: origin,
    destination_airport: destination,
    cabins: 'business',
    source: FLYING_BLUE_SOURCE,
    carriers: AIR_FRANCE_CARRIER,
    start_date: start,
    end_date: end,
    order_by: 'lowest_mileage',
    include_trips: 'true',
    minify_trips: 'true',
    take: '500',
  });

  if (error) return { deals: [], searchedAt: Date.now(), error };

  const deals = (data ?? [])
    .map((item) => dealFromAvailability(item, direction, buyRates))
    .filter((d): d is AwardDeal => d != null)
    .sort((a, b) => a.totalCostCad - b.totalCostCad || a.miles - b.miles);

  return { deals, searchedAt: Date.now() };
}

export async function searchBothRoutes(
  apiKey: string,
  dateRangeDays: number,
  buyRates: FlyingBlueBuyRates,
): Promise<{ montrealParis: SearchResult; parisMontreal: SearchResult }> {
  const [montrealParis, parisMontreal] = await Promise.all([
    searchRoute(
      apiKey,
      ROUTES.montrealParis.origin,
      ROUTES.montrealParis.destination,
      ROUTES.montrealParis.direction,
      dateRangeDays,
      buyRates,
    ),
    searchRoute(
      apiKey,
      ROUTES.parisMontreal.origin,
      ROUTES.parisMontreal.destination,
      ROUTES.parisMontreal.direction,
      dateRangeDays,
      buyRates,
    ),
  ]);

  return { montrealParis, parisMontreal };
}

export function getBestDeals(deals: AwardDeal[], limit = 5): AwardDeal[] {
  return [...deals].sort((a, b) => a.totalCostCad - b.totalCostCad).slice(0, limit);
}

export function formatDuration(minutes: number | null): string {
  if (minutes == null) return '—';
  const h = Math.floor(minutes / 60);
  const m = minutes % 60;
  return `${h}h${m.toString().padStart(2, '0')}`;
}

export function formatMiles(miles: number): string {
  return miles.toLocaleString('fr-CA');
}

const CONFIG_KEY = 'supremacy-award-flights-config';

export function loadConfig(): TrackerConfig {
  try {
    const raw = localStorage.getItem(CONFIG_KEY);
    if (!raw) return { ...DEFAULT_CONFIG };
    const parsed = JSON.parse(raw);
    // migration depuis ancienne config
    if (parsed.alertMaxMiles && !parsed.alertMaxTotalCad) {
      parsed.alertMaxTotalCad = DEFAULT_CONFIG.alertMaxTotalCad;
    }
    return { ...DEFAULT_CONFIG, ...parsed };
  } catch {
    return { ...DEFAULT_CONFIG };
  }
}

export function saveConfig(config: TrackerConfig): void {
  localStorage.setItem(CONFIG_KEY, JSON.stringify(config));
}

export { refreshFlyingBlueBuyRates };
export type { FlyingBlueBuyRates } from './flyingBluePurchase';
