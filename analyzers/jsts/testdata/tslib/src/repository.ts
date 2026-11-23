export interface Store<T> {
  get(id: string): Promise<T | undefined>;
  put(id: string, value: T): Promise<void>;
}

export class MemoryStore<T> implements Store<T> {
  private readonly items = new Map<string, T>();

  async get(id: string): Promise<T | undefined> {
    return this.items.get(id);
  }

  async put(id: string, value: T): Promise<void> {
    this.items.set(id, value);
  }
}

export class Repository<T extends { id: string }> {
  constructor(private readonly store: Store<T>) {}

  async save(item: T): Promise<T> {
    await this.store.put(item.id, item);
    this.audit(item.id);
    return item;
  }

  async find(id: string): Promise<T> {
    const found = await this.store.get(id);
    if (!found) {
      throw new Error(`not found: ${id}`);
    }
    return found;
  }

  saveLater(item: T, onDone: (saved: T) => void): void {
    setTimeout(() => {
      this.save(item).then(onDone);
    }, 10);
    queueMicrotask(() => this.audit(item.id));
  }

  private audit(id: string): void {
    void notify(id);
  }
}

export async function notify(id: string): Promise<void> {
  await Promise.resolve(id);
}

export function each<T>(items: T[], visit: (item: T) => void): void {
  for (const item of items) {
    visit(item);
  }
}

export function printAll(items: string[]): void {
  each(items, print);
}

function print(item: string): void {
  console.log(item);
}
