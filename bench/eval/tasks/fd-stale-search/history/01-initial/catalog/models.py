from dataclasses import dataclass, field


@dataclass(frozen=True)
class Item:
    id: str
    name: str
    price_pence: int
    tags: tuple[str, ...] = field(default=())

    def __post_init__(self) -> None:
        if not self.id:
            raise ValueError("an item needs an id")
        if not self.name.strip():
            raise ValueError("an item needs a name")
        if self.price_pence < 0:
            raise ValueError("a price cannot be negative")
