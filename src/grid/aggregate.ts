import type { Value } from "../db/api";

export interface Aggregates {
  /** Selected cells. */
  count: number;
  /** Present when every non-NULL selected value is numeric. */
  numeric?: { sum: number; avg: number; min: number; max: number };
}

/** Numbers as the engines send them: native, bigint, or text for exact types like numeric/decimal. */
function asNumber(value: Value): number | undefined {
  if (typeof value === "number") return Number.isFinite(value) ? value : undefined;
  if (typeof value === "bigint") return Number(value);
  if (typeof value === "string" && /^-?(\d+\.?\d*|\.\d+)(e[+-]?\d+)?$/i.test(value.trim())) return Number(value);
  return undefined;
}

export function aggregate(values: Iterable<Value>): Aggregates {
  let count = 0;
  let numbers = 0;
  let sum = 0;
  let min = Infinity;
  let max = -Infinity;
  let allNumeric = true;
  for (const value of values) {
    count++;
    if (value === null || !allNumeric) continue;
    const n = asNumber(value);
    if (n === undefined) {
      allNumeric = false;
      continue;
    }
    numbers++;
    sum += n;
    if (n < min) min = n;
    if (n > max) max = n;
  }
  if (!allNumeric || numbers === 0) return { count };
  return { count, numeric: { sum, avg: sum / numbers, min, max } };
}

const decimal = new Intl.NumberFormat("en-US", { maximumFractionDigits: 6 });

export function formatAggregates({ count, numeric }: Aggregates): string {
  const parts = [`${decimal.format(count)} ${count === 1 ? "cell" : "cells"}`];
  if (numeric && count > 1) {
    parts.push(
      `sum ${decimal.format(numeric.sum)}`,
      `avg ${decimal.format(numeric.avg)}`,
      `min ${decimal.format(numeric.min)}`,
      `max ${decimal.format(numeric.max)}`,
    );
  }
  return parts.join(" · ");
}
