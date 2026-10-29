/** Small deterministic PRNG (mulberry32). Not for cryptographic use. */
export class Prng {
  #state: number;

  constructor(seed: number) {
    this.#state = seed >>> 0;
  }

  /** Uniform float in [0, 1). */
  next(): number {
    let t = (this.#state = (this.#state + 0x6d2b79f5) >>> 0);
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  }

  /** Integer in [min, max] inclusive. */
  int(min: number, max: number): number {
    return min + Math.floor(this.next() * (max - min + 1));
  }

  pick<T>(items: readonly T[]): T {
    const item = items[Math.floor(this.next() * items.length)];
    if (item === undefined) throw new RangeError("pick() from empty list");
    return item;
  }

  chance(probability: number): boolean {
    return this.next() < probability;
  }
}
