#pragma once
#include "library_types.hpp"
#include <QJsonArray>
#include <QJsonObject>
#include <QRegularExpression>
#include <QSet>
#include <QStringList>

namespace aigle {
// Library search criteria, validation, matching, and ordering.
inline LibraryFilter libraryFilterForNewFolder(LibraryFilter filter) {
    filter.camera.clear(); filter.lens.clear(); filter.date.clear(); filter.keyword.clear();
    return filter;
}

inline bool validLibrarySearch(const QJsonObject& search) {
    if (search.value("name").toString().trimmed().isEmpty() || !search.value("filter").isObject() ||
        !search.value("sort").isObject())
        return false;
    const auto filter = search.value("filter").toObject();
    for (auto it = filter.begin(); it != filter.end(); ++it) {
        if (!QStringList{"query", "rating", "label", "camera", "lens", "date", "keyword"}.contains(
                it.key()))
            return false;
        if (it.key() != "rating" && it.key() != "label" && !it.value().isString() &&
            !it.value().isNull())
            return false;
    }
    const auto r = filter.value("rating"), l = filter.value("label");
    if (!r.isUndefined() &&
        !QStringList{"Any", "Unrated", "Picked", "Rejected"}.contains(r.toString())) {
        const auto obj = r.toObject();
        const auto value = obj.value("AtLeast");
        if (obj.size() != 1 || !value.isDouble() || value.toInt(-1) < 0 || value.toInt(-1) > 5 ||
            value.toDouble() != value.toInt(-1))
            return false;
    }
    if (!l.isUndefined() && l != "Any" && l != "None") {
        const auto obj = l.toObject();
        if (obj.size() != 1 || !obj.value("Is").isString())
            return false;
    }
    const auto sort = search.value("sort").toObject();
    return sort.size() == 2 && sort.value("descending").isBool() &&
           QStringList{"Name", "Rating", "Captured", "Modified"}.contains(
               sort.value("key").toString());
}
inline bool libraryStackVisible(bool matches, const QString& stack, bool collapse,
                                QSet<QString>& seen) {
    if (!matches || !collapse || stack.isEmpty())
        return matches;
    if (seen.contains(stack))
        return false;
    seen.insert(stack);
    return true;
}
inline bool libraryMatches(const QString& path, const LibraryPhoto& photo,
                           const LibraryFilter& filter) {
    using Rating = LibraryFilter::Rating;
    using Label = LibraryFilter::Label;
    const bool needsMeta = filter.rating != Rating::Any || filter.label != Label::Any || !filter.keyword.isEmpty();
    const bool needsFacts = !filter.camera.isEmpty() || !filter.lens.isEmpty() || !filter.date.isEmpty();
    if ((needsMeta && !photo.metadataLoaded) || (needsFacts && !photo.factsLoaded)) return false;
    const int stars = photo.rating.value_or(0);
    const auto flag = photo.flag == "none" && stars < 0 ? QString("rejected") : photo.flag;
    if (filter.rating == Rating::AtLeast && (stars < filter.minimumRating || flag == "rejected")) return false;
    if (filter.rating == Rating::Unrated && (stars != 0 || flag == "rejected")) return false;
    if (filter.rating == Rating::Picked && flag != "picked") return false;
    if (filter.rating == Rating::Rejected && flag != "rejected") return false;
    if (filter.label == Label::None && !photo.label.isEmpty()) return false;
    if (filter.label == Label::Is && photo.label.compare(filter.color, Qt::CaseInsensitive) != 0) return false;
    if (!filter.camera.isEmpty() && filter.camera != photo.camera) return false;
    if (!filter.lens.isEmpty() && filter.lens != photo.lens) return false;
    if (!photo.captured.value_or(QString{}).startsWith(filter.date)) return false;
    if (!filter.keyword.isEmpty() && !photo.keywords.contains(filter.keyword, Qt::CaseInsensitive)) return false;
    QStringList fields{path, photo.camera, photo.lens, photo.captured.value_or(QString{}), photo.label};
    fields.append(photo.tags);
    for (const auto& text : photo.iptc) fields.append(text);
    for (auto word : filter.query.split(QRegularExpression("\\s+"), Qt::SkipEmptyParts)) {
        const bool exclude = word.startsWith('-') && word.size() > 1;
        if (exclude) word.remove(0, 1);
        bool found = false;
        for (const auto& field : fields) found |= field.contains(word, Qt::CaseInsensitive);
        if (found == exclude) return false;
    }
    return true;
}
inline bool libraryLess(const QString& a, const LibraryPhoto& am, const QString& b,
                        const LibraryPhoto& bm, const QString& key, bool descending) {
    if (key != "Name") {
        const bool ak = key == "Rating" ? am.rating.has_value() : key == "Captured" ? am.captured.has_value() : am.modified.has_value();
        const bool bk = key == "Rating" ? bm.rating.has_value() : key == "Captured" ? bm.captured.has_value() : bm.modified.has_value();
        if (ak != bk) return ak; // Unknown values stay last in either direction.
        if (ak) {
            const auto compare = [&](const auto& av, const auto& bv) -> std::optional<bool> {
                if (av == bv) return {};
                return descending ? av > bv : av < bv;
            };
            const auto order = key == "Rating" ? compare(am.rating, bm.rating)
                : key == "Captured" ? compare(am.captured, bm.captured) : compare(am.modified, bm.modified);
            if (order) return *order;
        }
    }
    int cmp = QString::compare(a, b, Qt::CaseInsensitive);
    if (!cmp) cmp = QString::compare(a, b);
    return key == "Name" && descending ? cmp > 0 : cmp < 0;
}
} // namespace aigle
