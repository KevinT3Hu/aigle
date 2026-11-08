#pragma once
#include <QList>
#include <QMetaType>
#include <QPointF>
#include <QString>
#include <optional>
namespace aigle {
struct RetouchCorrection {
    std::optional<QString> mode = std::nullopt;
    std::optional<QPointF> target = std::nullopt, source = std::nullopt, center = std::nullopt;
    std::optional<double> radius = std::nullopt, feather = std::nullopt, opacity = std::nullopt, pupil = std::nullopt, darken = std::nullopt;
    bool empty() const { return !mode && !target && !source && !center && !radius && !feather && !opacity && !pupil && !darken; }
    double field(const QString& name) const {
        if (name == "radius") return radius.value_or(0);
        if (name == "feather") return feather.value_or(0);
        if (name == "opacity") return opacity.value_or(0);
        if (name == "pupil") return pupil.value_or(0);
        if (name == "darken") return darken.value_or(0);
        const auto point = name.startsWith("target.") ? target : name.startsWith("source.") ? source : center;
        return name.endsWith(".x") ? point.value_or(QPointF{}).x() : point.value_or(QPointF{}).y();
    }
};
struct RetouchParameters { QList<RetouchCorrection> spots, redEye; };
struct RetouchGuide {
    bool valid = false, placement = false;
    QString kind;
    int index = -1;
    RetouchCorrection value;
    QPointF target, radiusHandle;
    std::optional<QPointF> source;
    QList<QPointF> outline, sourceOutline;
};
struct RetouchResult {
    QString session;
    std::optional<QString> error;
    std::optional<RetouchGuide> guide;
    bool placement = false;
};
struct RetouchSelection { QString kind; int index = -1; bool operator==(const RetouchSelection&) const = default; };
}
Q_DECLARE_METATYPE(aigle::RetouchCorrection)
Q_DECLARE_METATYPE(aigle::RetouchSelection)
