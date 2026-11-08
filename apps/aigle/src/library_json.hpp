#pragma once
#include "library_types.hpp"
#include <QJsonArray>
#include <QJsonObject>
namespace aigle {
inline LibraryFilter libraryFilterFromJson(const QJsonObject& value) {
    LibraryFilter result;
    result.query = value["query"].toString();
    for (const auto* key : {"camera", "lens", "date", "keyword"}) result.setFacet(key, value[key].toString());
    const auto rating = value["rating"];
    if (rating == "Unrated") result.rating = LibraryFilter::Rating::Unrated;
    else if (rating == "Picked") result.rating = LibraryFilter::Rating::Picked;
    else if (rating == "Rejected") result.rating = LibraryFilter::Rating::Rejected;
    else if (rating.isObject()) {
        result.rating = LibraryFilter::Rating::AtLeast;
        result.minimumRating = rating.toObject()["AtLeast"].toInt();
    }
    const auto label = value["label"];
    if (label == "None") result.label = LibraryFilter::Label::None;
    else if (label.isObject()) {
        result.label = LibraryFilter::Label::Is;
        result.color = label.toObject()["Is"].toString();
    }
    return result;
}
inline QJsonObject toJson(const LibraryFilter& value) {
    QJsonObject result{{"query", value.query}};
    for (const auto* key : {"camera", "lens", "date", "keyword"})
        if (!value.facet(key).isEmpty()) result[key] = value.facet(key);
    switch (value.rating) {
    case LibraryFilter::Rating::Any: result["rating"] = "Any"; break;
    case LibraryFilter::Rating::Unrated: result["rating"] = "Unrated"; break;
    case LibraryFilter::Rating::Picked: result["rating"] = "Picked"; break;
    case LibraryFilter::Rating::Rejected: result["rating"] = "Rejected"; break;
    case LibraryFilter::Rating::AtLeast: result["rating"] = QJsonObject{{"AtLeast", value.minimumRating}}; break;
    }
    result["label"] = value.label == LibraryFilter::Label::Any ? QJsonValue("Any")
        : value.label == LibraryFilter::Label::None ? QJsonValue("None") : QJsonValue(QJsonObject{{"Is", value.color}});
    return result;
}
inline LibraryPhoto libraryPhotoFromJson(const QJsonObject& value) {
    LibraryPhoto result;
    result.metadataLoaded = value["meta_loaded"].toBool();
    result.factsLoaded = value["facts_loaded"].toBool();
    if (value["rating"].isDouble()) result.rating = value["rating"].toInt();
    if (value["captured"].isString()) result.captured = value["captured"].toString();
    if (value["modified"].isDouble()) result.modified = value["modified"].toDouble();
    result.flag = value["flag"].toString();
    result.label = value["label"].toString();
    result.camera = value["camera"].toString();
    result.lens = value["lens"].toString();
    for (const auto& tag : value["tags"].toArray()) result.tags.append(tag.toString());
    for (const auto& tag : value["keywords"].toArray()) result.keywords.append(tag.toString());
    const auto iptc = value["iptc"].toObject();
    for (auto it = iptc.begin(); it != iptc.end(); ++it) result.iptc[it.key()] = it.value().toString();
    return result;
}
}
