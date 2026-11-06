import Foundation
import SwiftData

@Model
final class KeptRepo {
    @Attribute(.unique) var id: String
    var name: String
    var machine: String
    var status: String
    var since: Double
    var order: Int

    init(id: String, name: String, machine: String, status: String, since: Double, order: Int) {
        self.id = id
        self.name = name
        self.machine = machine
        self.status = status
        self.since = since
        self.order = order
    }
}

@Model
final class KeptItem {
    #Unique<KeptItem>([\.repo, \.seq])
    var repo: String
    var seq: Int
    var body: Data

    init(repo: String, seq: Int, body: Data) {
        self.repo = repo
        self.seq = seq
        self.body = body
    }
}

@Model
final class Queued {
    @Attribute(.unique) var id: String
    var repo: String
    var text: String
    var at: Date

    init(id: String, repo: String, text: String, at: Date) {
        self.id = id
        self.repo = repo
        self.text = text
        self.at = at
    }
}

@MainActor
final class Keep {
    let context: ModelContext

    init(memory: Bool) {
        let schema = Schema([KeptRepo.self, KeptItem.self, Queued.self])
        let container = try! ModelContainer(for: schema, configurations: ModelConfiguration(isStoredInMemoryOnly: memory))
        context = ModelContext(container)
    }

    func repos() -> [KeptRepo] {
        (try? context.fetch(FetchDescriptor<KeptRepo>(sortBy: [SortDescriptor(\.order)]))) ?? []
    }

    func items(_ repo: String) -> [Item] {
        let fetch = FetchDescriptor<KeptItem>(predicate: #Predicate { $0.repo == repo }, sortBy: [SortDescriptor(\.seq)])
        return ((try? context.fetch(fetch)) ?? []).compactMap { try? JSONDecoder().decode(Item.self, from: $0.body) }
    }

    func queued() -> [Queued] {
        (try? context.fetch(FetchDescriptor<Queued>(sortBy: [SortDescriptor(\.at)]))) ?? []
    }

    func put(_ item: Item, repo: String) {
        guard let body = try? JSONEncoder().encode(item) else { return }
        context.insert(KeptItem(repo: repo, seq: item.seq, body: body))
    }

    func save() {
        try? context.save()
    }

    func wipe() {
        try? context.delete(model: KeptRepo.self)
        try? context.delete(model: KeptItem.self)
        try? context.delete(model: Queued.self)
        save()
    }
}
