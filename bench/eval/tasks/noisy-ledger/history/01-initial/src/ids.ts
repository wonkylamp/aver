// Transaction ids: the date plus a short random suffix, so ids sort by date
// and are readable in exports ("tx-20260314-k3f").
const ALPHABET = "0123456789abcdefghijklmnopqrstuvwxyz";

export function newId(date: string): string {
  let suffix = "";
  for (let i = 0; i < 3; i++) suffix += ALPHABET[Math.floor(Math.random() * ALPHABET.length)];
  return `tx-${date.replace(/-/g, "")}-${suffix}`;
}

export function idDate(id: string): string | null {
  const m = /^tx-(\d{4})(\d{2})(\d{2})-[0-9a-z]+$/.exec(id);
  return m ? `${m[1]}-${m[2]}-${m[3]}` : null;
}
