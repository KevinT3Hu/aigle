#pragma once
#include <QMap>
#include <QStringList>
#include <optional>
namespace aigle {
struct LibraryFilter {
    enum class Rating { Any, Unrated, Picked, Rejected, AtLeast };
    enum class Label { Any, None, Is };
    QString query, camera, lens, date, keyword;
    Rating rating = Rating::Any;
    int minimumRating = 0;
    Label label = Label::Any;
    QString color;
    QString facet(const QString& key) const {
        return key == "camera" ? camera : key == "lens" ? lens : key == "date" ? date : keyword;
    }
    void setFacet(const QString& key, const QString& value) {
        if (key == "camera") camera = value;
        else if (key == "lens") lens = value;
        else if (key == "date") date = value;
        else if (key == "keyword") keyword = value;
    }
    bool operator==(const LibraryFilter&) const = default;
};
struct LibraryPhoto {
    bool metadataLoaded = false, factsLoaded = false;
    std::optional<int> rating;
    QString flag, label, camera, lens;
    std::optional<QString> captured;
    std::optional<double> modified;
    QStringList keywords, tags;
    QMap<QString, QString> iptc;
};
}
