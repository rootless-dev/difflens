import { round } from "@lib/util";

export interface Shape {
  area(): number;
  readonly name: string;
}

export abstract class Base implements Shape {
  abstract area(): number;
  abstract get name(): string;

  describe(): string {
    return `${this.name} with area ${round(this.area())}`;
  }
}

export class Circle extends Base {
  constructor(private readonly radius: number) {
    super();
  }

  area(): number {
    return Math.PI * this.radius * this.radius;
  }

  get name(): string {
    return "circle";
  }
}

export class Square extends Base {
  #side = 0;

  constructor(side: number) {
    super();
    this.side = side;
  }

  area(): number {
    return this.#side * this.#side;
  }

  get name(): string {
    return "square";
  }

  set side(value: number) {
    if (value < 0 || Number.isNaN(value)) {
      throw new Error("negative side");
    }
    this.#side = value;
  }
}

export function totalArea(shapes: Shape[]): number {
  let total = 0;
  for (const shape of shapes) {
    total += shape.area();
  }
  return total;
}

export function largest(shapes: Shape[]): Shape | undefined {
  return shapes.reduce<Shape | undefined>((best, shape) => (best && best.area() >= shape.area() ? best : shape), undefined);
}
