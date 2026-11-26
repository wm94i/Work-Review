export function getIsoWeekKey(date: Date): string {
  const target = new Date(date.getFullYear(), date.getMonth(), date.getDate(), 12, 0, 0);
  const dayNum = (target.getDay() + 6) % 7;
  target.setDate(target.getDate() - dayNum + 3);
  const isoYear = target.getFullYear();
  const jan4 = new Date(isoYear, 0, 4, 12, 0, 0);
  const jan4DayNum = (jan4.getDay() + 6) % 7;
  const week1Monday = new Date(isoYear, 0, 4 - jan4DayNum, 12, 0, 0);
  const diffDays = Math.round((target.getTime() - week1Monday.getTime()) / 86_400_000);
  const isoWeek = Math.floor(diffDays / 7) + 1;
  return `${isoYear}-W${String(isoWeek).padStart(2, '0')}`;
}

export function shiftIsoWeekKey(weekKey: string, deltaWeeks: number): string {
  const match = /^(\d{4})-W(\d{2})$/.exec(weekKey);
  if (!match) return getIsoWeekKey(new Date());
  const year = Number(match[1]);
  const week = Number(match[2]);
  const jan4 = new Date(year, 0, 4, 12, 0, 0);
  const jan4DayNum = (jan4.getDay() + 6) % 7;
  const monday = new Date(year, 0, 4 - jan4DayNum + (week - 1 + deltaWeeks) * 7, 12, 0, 0);
  return getIsoWeekKey(monday);
}

export function getMonthKey(date: Date): string {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, '0')}`;
}

export function shiftMonthKey(monthKey: string, deltaMonths: number): string {
  const match = /^(\d{4})-(\d{2})$/.exec(monthKey);
  if (!match) return getMonthKey(new Date());
  const year = Number(match[1]);
  const month = Number(match[2]);
  const shifted = new Date(year, month - 1 + deltaMonths, 1, 12, 0, 0);
  return getMonthKey(shifted);
}
