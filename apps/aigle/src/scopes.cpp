#include "commands.hpp"
#include "localization.hpp"
#include "scopes.hpp"
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QPainter>
#include <QPainterPath>
#include <QPushButton>
#include <QSettings>
#include <QToolButton>
#include <QUuid>
#include <QVBoxLayout>
#include <QWhatsThis>
#include <algorithm>
#include <cmath>
#include <numeric>

namespace aigle {
namespace {
constexpr int densitySize = 64;
const std::array<QColor, 4> colors{QColor(255, 80, 80, 190), QColor(80, 255, 80, 190),
                                   QColor(80, 144, 255, 190), QColor(220, 220, 220, 205)};
double pq(double nits) {
    const auto p = std::pow(std::max(0.0, nits) / 10000.0, 2610.0 / 16384.0);
    return std::pow((3424.0 / 4096.0 + 2413.0 / 128.0 * p) / (1.0 + 2392.0 / 128.0 * p),
                    2523.0 / 32.0);
}
double nits(double level) {
    const auto p = std::pow(std::max(0.0, level) * pq(1000.0), 32.0 / 2523.0);
    return 10000.0 *
           std::pow(std::max(0.0, p - 3424.0 / 4096.0) / (2413.0 / 128.0 - 2392.0 / 128.0 * p),
                    16384.0 / 2610.0);
}
QString levelText(double level, bool hdr) {
    return hdr ? QObject::tr("%1 cd/m²").arg(nits(level), 0, 'f', 1)
               : QObject::tr("%1 / 255").arg(level * 255.0, 0, 'f', 1);
}
} // namespace
ScopeGraph::ScopeGraph(QWidget* parent) : QWidget(parent) {
    setObjectName("scopeGraph");
    bindTranslation(this, [this] {
        setAccessibleName(tr("Image scope graph"));
        if (hovering_) {
            const auto text = readout(hover_);
            setToolTip(text);
            if (hovered) hovered(text);
        }
        update();
    });
    setMouseTracking(true);
    setFocusPolicy(Qt::StrongFocus);
    setMinimumHeight(128);
    setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);
}
QRectF ScopeGraph::plotRect() const {
    QRectF area = QRectF(rect()).adjusted(3, 5, -3, -20);
    if (mode_ == 6) {
        const auto side = std::max(1.0, std::min(area.width(), area.height()));
        area = QRectF(area.center().x() - side / 2, area.top(), side, side);
    }
    return area;
}
void ScopeGraph::setData(const ScopeResult& reply) {
    data_ = reply.data;
    targets_ = reply.targets;
    white_ = reply.diffuseWhite;
    rebuildDensity();
    update();
}
void ScopeGraph::configure(int mode, bool logarithmic, std::array<bool, 3> channels) {
    const bool rebuild = logarithmic_ != logarithmic;
    mode_ = std::clamp(mode, 0, 6);
    logarithmic_ = logarithmic;
    channels_ = channels;
    if (rebuild)
        rebuildDensity();
    hovering_ = false;
    update();
}
void ScopeGraph::rebuildDensity() {
    const auto wave = data_.wave;
    for (size_t c = 0; c < densities_.size(); ++c) {
        const auto values =
            c == 4 ? data_.vector
                   : wave.at(c);
        auto& image = densities_[c];
        image = QImage(densitySize, densitySize, QImage::Format_ARGB32);
        image.fill(Qt::transparent);
        double peak = 1;
        for (const auto& value : values)
            peak = std::max(peak, value);
        for (qsizetype i = 0; i < std::min<qsizetype>(values.size(), densitySize * densitySize);
             ++i) {
            const auto value = values.at(i);
            if (value <= 0)
                continue;
            const double alpha =
                logarithmic_ ? std::log1p(value) / std::log1p(peak) : std::sqrt(value / peak);
            auto color = c == 4 ? QColor(224, 192, 128) : colors[c];
            color.setAlphaF(float(std::clamp(alpha, 0.0, 1.0)));
            image.setPixelColor(int(i % densitySize), int(i / densitySize), color);
        }
    }
}
bool ScopeGraph::event(QEvent* event) {
    if (event->type() == QEvent::ShortcutOverride) {
        const auto* key = static_cast<QKeyEvent*>(event);
        if (!(key->modifiers() & (Qt::ControlModifier | Qt::AltModifier | Qt::MetaModifier)) &&
            (key->key() == Qt::Key_Left || key->key() == Qt::Key_Right ||
             key->key() == Qt::Key_Up || key->key() == Qt::Key_Down || key->key() == Qt::Key_Home ||
             key->key() == Qt::Key_End || key->key() == Qt::Key_Escape)) {
            event->accept();
            return true;
        }
    }
    return QWidget::event(event);
}
void ScopeGraph::keyPressEvent(QKeyEvent* event) {
    if (!hovering_)
        hover_ = {0.5, 0.5};
    const auto step = mode_ >= 4 ? 1.0 / 63.0 : 1.0 / 255.0;
    switch (event->key()) {
    case Qt::Key_Left:
        hover_.rx() -= step;
        break;
    case Qt::Key_Right:
        hover_.rx() += step;
        break;
    case Qt::Key_Up:
        hover_.ry() -= step;
        break;
    case Qt::Key_Down:
        hover_.ry() += step;
        break;
    case Qt::Key_Home:
        hover_.setX(0);
        break;
    case Qt::Key_End:
        hover_.setX(1);
        break;
    case Qt::Key_Escape:
        leaveEvent(nullptr);
        event->accept();
        return;
    default:
        QWidget::keyPressEvent(event);
        return;
    }
    hover_.setX(std::clamp(hover_.x(), 0.0, 1.0));
    hover_.setY(std::clamp(hover_.y(), 0.0, 1.0));
    hovering_ = true;
    const auto text = readout(hover_);
    setAccessibleDescription(text);
    if (hovered)
        hovered(text);
    update();
    event->accept();
}
void ScopeGraph::paintEvent(QPaintEvent*) {
    QPainter p(this);
    p.fillRect(rect(), QColor("#181818"));
    const auto area = plotRect();
    if (area.width() <= 0 || area.height() <= 0)
        return;
    p.setPen(QColor(255, 255, 255, 28));
    for (double f : {0.25, 0.5, 0.75})
        p.drawLine(QPointF(area.left(), area.top() + f * area.height()),
                   QPointF(area.right(), area.top() + f * area.height()));
    if (!data_.available) {
        p.setPen(palette().placeholderText().color());
        p.drawText(area, Qt::AlignCenter, tr("No scope data"));
        return;
    }
    p.save();
    p.setClipRect(area);
    if (mode_ >= 4) {
        if (mode_ == 4)
            p.drawImage(area, densities_[3]);
        else if (mode_ == 6)
            p.drawImage(area, densities_[4]);
        else
            for (int c = 0; c < 3; ++c)
                if (channels_[size_t(c)])
                    p.drawImage(QRectF(area.left() + area.width() * c / 3.0, area.top(),
                                       area.width() / 3.0, area.height()),
                                densities_[size_t(c)]);
    } else {
        const auto bins = data_.bins;
        const bool luma = mode_ == 1 || mode_ == 3;
        double peak = 1;
        for (int c = luma ? 3 : 0; c < (luma ? 4 : 3); ++c) {
            if (c < 3 && !channels_[size_t(c)])
                continue;
            const auto values = bins.at(size_t(c));
            for (qsizetype i = 1; i < std::min<qsizetype>(255, values.size()); ++i)
                peak = std::max(peak, values.at(i));
        }
        p.setRenderHint(QPainter::Antialiasing);
        for (int c = luma ? 3 : 0; c < (luma ? 4 : 3); ++c) {
            if (c < 3 && !channels_[size_t(c)])
                continue;
            const auto values = bins.at(size_t(c));
            const auto width = mode_ == 2 ? area.width() / 3.0 : area.width();
            const auto left = area.left() + (mode_ == 2 ? width * c : 0);
            double total = 0;
            for (const auto& v : values)
                total += v;
            QPainterPath shape;
            shape.moveTo(left, area.bottom());
            double sum = 0;
            for (qsizetype i = 0; i < values.size(); ++i) {
                const auto value = values.at(i);
                sum += value;
                double height = mode_ == 3     ? sum / std::max(1.0, total)
                                : logarithmic_ ? std::log1p(value) / std::log1p(peak)
                                               : value / peak;
                shape.lineTo(left + width * double(i) /
                                        double(std::max<qsizetype>(1, values.size() - 1)),
                             area.bottom() - area.height() * std::clamp(height, 0.0, 1.0));
            }
            shape.lineTo(left + width, area.bottom());
            shape.closeSubpath();
            p.fillPath(shape, colors[size_t(c)]);
        }
    }
    if (mode_ == 6) {
        p.setPen(QColor(255, 255, 255, 70));
        p.drawLine(QPointF(area.center().x(), area.top()),
                   QPointF(area.center().x(), area.bottom()));
        p.drawLine(QPointF(area.left(), area.center().y()),
                   QPointF(area.right(), area.center().y()));
        QPolygonF polygon;
        const std::array<QColor, 6> targets{Qt::red,  Qt::yellow, Qt::green,
                                            Qt::cyan, Qt::blue,   Qt::magenta};
        for (qsizetype i = 0; i < targets_.size(); ++i) {
            const auto uv = targets_.at(i);
            const QPointF point(area.left() + area.width() * (uv.x() + 0.5),
                                area.top() + area.height() * (0.5 - uv.y()));
            polygon.append(point);
            p.fillRect(QRectF(point - QPointF(2, 2), QSizeF(4, 4)),
                       targets[size_t(i) % targets.size()]);
        }
        p.drawPolygon(polygon);
        p.setPen(QColor(255, 192, 144, 170));
        const auto angle = 123.0 * std::acos(-1) / 180.0;
        p.drawLine(area.center(),
                   QPointF(area.left() + area.width() * (0.5 + 0.48 * std::cos(angle)),
                           area.top() + area.height() * (0.5 - 0.48 * std::sin(angle))));
    } else if (data_.hdr) {
        p.setPen(QColor(255, 206, 96, 180));
        if (mode_ >= 4)
            p.drawLine(QPointF(area.left(), area.bottom() - area.height() * white_),
                       QPointF(area.right(), area.bottom() - area.height() * white_));
        else
            for (int c = 0; c < (mode_ == 2 ? 3 : 1); ++c) {
                const auto x = area.left() + area.width() * (c + white_) / (mode_ == 2 ? 3.0 : 1.0);
                p.drawLine(QPointF(x, area.top()), QPointF(x, area.bottom()));
            }
    }
    if (hovering_) {
        p.setPen(QColor(255, 255, 255, 110));
        const auto x = area.left() + area.width() * hover_.x(),
                   y = area.top() + area.height() * hover_.y();
        p.drawLine(QPointF(x, area.top()), QPointF(x, area.bottom()));
        p.drawLine(QPointF(area.left(), y), QPointF(area.right(), y));
    }
    p.restore();
    p.setPen(palette().placeholderText().color());
    const auto label = mode_ == 6                    ? tr("Cb / Cr −0.5 … +0.5")
                       : mode_ == 3                  ? tr("Cumulative 0–100%")
                       : data_.hdr ? tr("PQ · 0–1000 cd/m²")
                                                     : tr("SDR · 0–255");
    p.drawText(QRectF(0, height() - 18, width(), 18), Qt::AlignCenter, label);
    if (hasFocus()) {
        p.setPen(QPen(palette().highlight().color(), 1, Qt::DashLine));
        p.drawRect(rect().adjusted(1, 1, -2, -2));
    }
}
QString ScopeGraph::readout(const QPointF& point) const {
    if (!data_.available)
        return {};
    if (mode_ == 6)
        return tr("Cb %1 · Cr %2").arg(point.x() - 0.5, 0, 'f', 3).arg(0.5 - point.y(), 0, 'f', 3);
    const auto localX = (mode_ == 2 || mode_ == 5)
                            ? point.x() * 3.0 - std::min(2, int(point.x() * 3.0))
                            : point.x();
    const bool hdr = data_.hdr;
    if (mode_ >= 4)
        return tr("Image x %1% · %2")
            .arg(localX * 100, 0, 'f', 1)
            .arg(levelText(1 - point.y(), hdr));
    const auto index = std::clamp(int(std::round(localX * 255)), 0, 255);
    const auto bins = data_.bins;
    if (mode_ == 3) {
        double sum = 0, total = 0;
        const auto values = bins.at(3);
        for (qsizetype i = 0; i < values.size(); ++i) {
            total += values.at(i);
            if (i <= index)
                sum += values.at(i);
        }
        return tr("%1 · %2% below")
            .arg(levelText(localX, hdr))
            .arg(sum / std::max(1.0, total) * 100, 0, 'f', 1);
    }
    QStringList counts;
    for (int c = mode_ == 1 ? 3 : 0; c < (mode_ == 1 ? 4 : 3); ++c) {
        if (c < 3 &&
            (!channels_[size_t(c)] || (mode_ == 2 && c != std::min(2, int(point.x() * 3)))))
            continue;
        counts.append(QString("%1 %2")
                          .arg(QStringList{"R", "G", "B", "Y"}.at(c))
                          .arg(qint64(bins.at(size_t(c)).value(index))));
    }
    return levelText(localX, hdr) + " · " + counts.join(" / ");
}
void ScopeGraph::mouseMoveEvent(QMouseEvent* event) {
    const auto area = plotRect();
    hovering_ = area.contains(event->position());
    if (hovering_)
        hover_ = {(event->position().x() - area.left()) / area.width(),
                  (event->position().y() - area.top()) / area.height()};
    if (hovered)
        hovered(hovering_ ? readout(hover_) : QString());
    update();
}
void ScopeGraph::leaveEvent(QEvent*) {
    hovering_ = false;
    if (hovered)
        hovered({});
    update();
}

ScopeThumbnail::ScopeThumbnail(QWidget* parent) : QWidget(parent) {
    setObjectName("scopeRegionImage");
    bindTranslation(this, [this] {
    setAccessibleName(tr("Scope measurement region"));
    setToolTip(tr(
        "Drag a region or click one sample cell. Arrows move the region; Shift+arrows resize; Home "
        "selects the whole crop. Escape cancels a drag. This SDR thumbnail is an analysis aid."));
    });
    setFocusPolicy(Qt::StrongFocus);
    setFixedHeight(80);
}
QRectF ScopeThumbnail::imageRect() const {
    if (sample_.isNull())
        return {};
    const auto size = sample_.size().scaled(this->size(), Qt::KeepAspectRatio);
    return QRectF(QPointF((width() - size.width()) / 2.0, (height() - size.height()) / 2.0),
                  QSizeF(size));
}
void ScopeThumbnail::setSample(const QImage& image) {
    if (image.size() != sample_.size()) {
        selection_ = {};
        dragging_ = false;
    }
    sample_ = image;
    update();
}
void ScopeThumbnail::wholeCrop() {
    selection_ = {};
    dragging_ = false;
    if (selected)
        selected(selection_);
    update();
}
QPoint ScopeThumbnail::cell(const QPointF& point) const {
    const auto area = imageRect();
    return {std::clamp(int((point.x() - area.left()) / area.width() * sample_.width()), 0,
                       sample_.width() - 1),
            std::clamp(int((point.y() - area.top()) / area.height() * sample_.height()), 0,
                       sample_.height() - 1)};
}
void ScopeThumbnail::drag(const QPointF& point) {
    selection_ = QRect(anchor_, cell(point)).normalized();
    if (selected)
        selected(selection_);
    update();
}
bool ScopeThumbnail::event(QEvent* event) {
    if (event->type() == QEvent::ShortcutOverride) {
        const auto* key = static_cast<QKeyEvent*>(event);
        if (!(key->modifiers() & (Qt::ControlModifier | Qt::AltModifier | Qt::MetaModifier)) &&
            (key->key() == Qt::Key_Left || key->key() == Qt::Key_Right ||
             key->key() == Qt::Key_Up || key->key() == Qt::Key_Down || key->key() == Qt::Key_Home ||
             key->key() == Qt::Key_Escape)) {
            event->accept();
            return true;
        }
    }
    return QWidget::event(event);
}
void ScopeThumbnail::paintEvent(QPaintEvent*) {
    QPainter p(this);
    p.fillRect(rect(), QColor("#181818"));
    const auto area = imageRect();
    if (sample_.isNull())
        return;
    p.drawImage(area, sample_);
    if (!selection_.isEmpty()) {
        const auto sx = area.width() / sample_.width(), sy = area.height() / sample_.height();
        p.setPen(QPen(QColor("#8aaed4"), 1));
        p.drawRect(QRectF(area.left() + selection_.x() * sx, area.top() + selection_.y() * sy,
                          selection_.width() * sx, selection_.height() * sy)
                       .adjusted(0, 0, -1, -1));
    }
    if (hasFocus()) {
        p.setPen(QPen(palette().highlight().color(), 1, Qt::DashLine));
        p.drawRect(rect().adjusted(1, 1, -2, -2));
    }
}
void ScopeThumbnail::mousePressEvent(QMouseEvent* event) {
    if (event->button() != Qt::LeftButton || sample_.isNull() ||
        !imageRect().contains(event->position()))
        return;
    setFocus();
    beforeDrag_ = selection_;
    anchor_ = cell(event->position());
    dragging_ = true;
    drag(event->position());
}
void ScopeThumbnail::mouseMoveEvent(QMouseEvent* event) {
    if (dragging_)
        drag(event->position());
}
void ScopeThumbnail::mouseReleaseEvent(QMouseEvent* event) {
    if (event->button() == Qt::LeftButton && dragging_) {
        drag(event->position());
        dragging_ = false;
    }
}
void ScopeThumbnail::cancelDrag() {
    if (!dragging_)
        return;
    dragging_ = false;
    selection_ = beforeDrag_;
    if (selected)
        selected(selection_);
    update();
}
void ScopeThumbnail::focusOutEvent(QFocusEvent* event) {
    cancelDrag();
    QWidget::focusOutEvent(event);
    update();
}
void ScopeThumbnail::keyPressEvent(QKeyEvent* event) {
    if (event->key() == Qt::Key_Escape) {
        cancelDrag();
        event->accept();
        return;
    }
    if (event->key() == Qt::Key_Home) {
        wholeCrop();
        event->accept();
        return;
    }
    if (sample_.isNull())
        return;
    QPoint delta;
    switch (event->key()) {
    case Qt::Key_Left:
        delta = {-1, 0};
        break;
    case Qt::Key_Right:
        delta = {1, 0};
        break;
    case Qt::Key_Up:
        delta = {0, -1};
        break;
    case Qt::Key_Down:
        delta = {0, 1};
        break;
    default:
        QWidget::keyPressEvent(event);
        return;
    }
    if (selection_.isEmpty())
        selection_ = sample_.rect();
    if (event->modifiers() & Qt::ShiftModifier)
        selection_.setSize(
            {std::clamp(selection_.width() + delta.x(), 1, sample_.width() - selection_.x()),
             std::clamp(selection_.height() + delta.y(), 1, sample_.height() - selection_.y())});
    else
        selection_.moveTopLeft(
            {std::clamp(selection_.x() + delta.x(), 0, sample_.width() - selection_.width()),
             std::clamp(selection_.y() + delta.y(), 0, sample_.height() - selection_.height())});
    if (selected)
        selected(selection_);
    update();
    event->accept();
}

Scopes::Scopes(QWidget* parent)
    : QWidget(parent), graph_(new ScopeGraph), thumbnail_(new ScopeThumbnail), mode_(new QComboBox),
      target_(new QComboBox), log_(new QCheckBox(tr("Log"))), region_(new QCheckBox(tr("Region"))),
      statistics_(new QLabel), readout_(new QLabel) {
    setObjectName("imageScopes");
    auto* layout = new QVBoxLayout(this);
    layout->setContentsMargins(12, 10, 12, 10);
    layout->setSpacing(6);
    auto* heading = new QHBoxLayout;
    mode_->setObjectName("scopeMode");
    mode_->setAccessibleName(tr("Scope type"));
    mode_->addItems({tr("RGB histogram"), tr("Luminance"), tr("RGB channels"),
                     tr("Cumulative luma"), tr("Luma waveform"), tr("RGB parade"),
                     tr("Vectorscope")});
    mode_->setItemData(3, tr("Cumulative luminance distribution, 0–100%"), Qt::ToolTipRole);
    mode_->setItemData(4, tr("Luminance waveform by image column"), Qt::ToolTipRole);
    heading->addWidget(mode_, 1);
    auto* help = new QToolButton;
    help->setText("?");
    help->setAccessibleName(tr("Scope measurement help"));
    const auto explanation = [this] { return tr("Scopes measure the edited crop. Full-crop histogram bins are exact at the histogram "
           "render resolution. Spatial plots, regions and percentages use at most 128×128 samples "
           "and exclude transparent pixels. Tiny features can be missed.\n\nHDR uses PQ normalized "
           "to 1000 cd/m²; the gold line is 203 cd/m² diffuse white. Vectorscope targets and the "
           "123° skin-hue line are references, not skin detection or gamut proof.\n\nBlack/Peak "
           "are output-boundary percentages, not RAW or pre-output clipping. Gamut counts negative "
           "linear components in the chosen target, independently of brightness; SDR output cannot "
           "recover already-clipped colors."); };
    help->setToolTip(explanation());
    help->setWhatsThis(explanation());
    graph_->setToolTip(
        tr("Hover or focus this graph and use arrows, Home or End for measurement readouts.") +
        "\n\n" + explanation());
    connect(help, &QToolButton::clicked, this, [help, explanation] {
        QWhatsThis::showText(help->mapToGlobal(QPoint(0, help->height())), explanation(), help);
    });
    heading->addWidget(help);
    layout->addLayout(heading);
    layout->addWidget(graph_);
    auto* controls = new QHBoxLayout;
    controls->addWidget(log_);
    QSettings settings;
    log_->setObjectName("scopeLog");
    log_->setChecked(settings.value("scopes/log", false).toBool());
    for (size_t c = 0; c < channels_.size(); ++c) {
        auto* button = new QToolButton;
        channels_[c] = button;
        button->setText(QStringList{"R", "G", "B"}.at(int(c)));
        button->setAccessibleName(tr("Show %1 channel").arg(button->text()));
        button->setCheckable(true);
        button->setChecked(settings.value(QString("scopes/channel%1").arg(c), true).toBool());
        controls->addWidget(button);
        connect(button, &QToolButton::toggled, this, [this] { configure(); });
    }
    controls->addStretch();
    layout->addLayout(controls);
    auto* targetRow = new QHBoxLayout;
    auto* gamutLabel = new QLabel(tr("Gamut"));
    targetRow->addWidget(gamutLabel);
    target_->setObjectName("scopeGamutTarget");
    target_->setAccessibleName(tr("Analysis gamut target"));
    target_->addItems({"sRGB", "Display P3", "Rec.2020"});
    targetRow->addWidget(target_, 1);
    layout->addLayout(targetRow);
    auto* regionRow = new QHBoxLayout;
    region_->setObjectName("scopeRegion");
    regionRow->addWidget(region_);
    auto* whole = new QPushButton(tr("Whole crop"));
    bindTranslation(whole, [this, whole, gamutLabel] {
        const QSignalBlocker blocker(mode_);
        const QStringList labels{tr("RGB histogram"), tr("Luminance"), tr("RGB channels"),
            tr("Cumulative luma"), tr("Luma waveform"), tr("RGB parade"), tr("Vectorscope")};
        for (int i = 0; i < labels.size(); ++i) mode_->setItemText(i, labels[i]);
        mode_->setAccessibleName(tr("Scope type"));
        mode_->setItemData(3, tr("Cumulative luminance distribution, 0–100%"), Qt::ToolTipRole);
        mode_->setItemData(4, tr("Luminance waveform by image column"), Qt::ToolTipRole);
        log_->setText(tr("Log"));
        region_->setText(tr("Region"));
        whole->setText(tr("Whole crop"));
        gamutLabel->setText(tr("Gamut"));
        target_->setAccessibleName(tr("Analysis gamut target"));
        for (auto* channel : channels_)
            channel->setAccessibleName(tr("Show %1 channel").arg(channel->text()));
        graph_->update();
        renderStatistics();
    });
    whole->setObjectName("scopeWholeCrop");
    regionRow->addWidget(whole, 1);
    layout->addLayout(regionRow);
    layout->addWidget(thumbnail_);
    thumbnail_->hide();
    statistics_->setObjectName("scopeStatistics");
    statistics_->setWordWrap(true);
    statistics_->setTextFormat(Qt::PlainText);
    bindTranslation(help, [this, help, explanation] {
        help->setAccessibleName(tr("Scope measurement help"));
        help->setToolTip(explanation());
        help->setWhatsThis(explanation());
        statistics_->setToolTip(explanation());
        graph_->setToolTip(tr("Hover or focus this graph and use arrows, Home or End for measurement readouts.") + "\n\n" + explanation());
    });
    layout->addWidget(statistics_);
    readout_->setObjectName("scopeReadout");
    readout_->setWordWrap(true);
    readout_->setTextFormat(Qt::PlainText);
    readout_->setMinimumHeight(28);
    layout->addWidget(readout_);
    mode_->setCurrentIndex(std::clamp(settings.value("scopes/mode", 0).toInt(), 0, 6));
    target_->setCurrentIndex(std::clamp(settings.value("scopes/target", 0).toInt(), 0, 2));
    graph_->hovered = [this](const QString& text) { readout_->setText(text); };
    thumbnail_->selected = [this](const QRect& roi) {
        roi_ = roi;
        schedule();
    };
    connect(mode_, &QComboBox::currentIndexChanged, this, [this] { configure(); });
    connect(log_, &QCheckBox::toggled, this, [this] { configure(); });
    connect(target_, &QComboBox::currentIndexChanged, this, [this] {
        savePreferences();
        schedule();
    });
    connect(region_, &QCheckBox::toggled, this, [this](bool enabled) {
        thumbnail_->setVisible(enabled);
        thumbnail_->wholeCrop();
    });
    connect(whole, &QPushButton::clicked, thumbnail_, &ScopeThumbnail::wholeCrop);
    timer_.setSingleShot(true);
    timer_.setInterval(60);
    connect(&timer_, &QTimer::timeout, this, [this] {
        if (!available_ || !dirty_ || !inFlight_.isEmpty())
            return;
        dirty_ = false;
        inFlight_ = QUuid::createUuid().toString(QUuid::WithoutBraces);
        std::optional<QRectF> roi;
        if (region_->isChecked() && !roi_.isEmpty() && !sampleSize_.isEmpty())
            roi = QRectF(QPointF(double(roi_.left()) / sampleSize_.width(), double(roi_.top()) / sampleSize_.height()),
                         QPointF(double(roi_.x() + roi_.width()) / sampleSize_.width(), double(roi_.y() + roi_.height()) / sampleSize_.height()));
        emit command(commands::Scopes{.request_id = inFlight_, .target = target_->currentIndex(), .roi = roi});
    });
    configure();
    reset();
}
void Scopes::configure() {
    graph_->configure(
        mode_->currentIndex(), log_->isChecked(),
        {channels_[0]->isChecked(), channels_[1]->isChecked(), channels_[2]->isChecked()});
    readout_->clear();
    savePreferences();
}
void Scopes::savePreferences() {
    QSettings settings;
    settings.setValue("scopes/mode", mode_->currentIndex());
    settings.setValue("scopes/log", log_->isChecked());
    settings.setValue("scopes/target", target_->currentIndex());
    for (size_t c = 0; c < channels_.size(); ++c)
        settings.setValue(QString("scopes/channel%1").arg(c), channels_[c]->isChecked());
}
void Scopes::reset() {
    timer_.stop();
    inFlight_.clear();
    dirty_ = false;
    available_ = false;
    roi_ = {};
    sampleSize_ = {};
    graph_->setData({});
    thumbnail_->setSample({});
    statisticsState_ = StatisticsState::Waiting;
    renderStatistics();
    readout_->clear();
}
void Scopes::histogramChanged(const Histogram& histogram) {
    if (!histogram.available)
        return;
    available_ = true;
    schedule();
}
void Scopes::schedule() {
    dirty_ = true;
    if (available_) {
        statisticsState_ = StatisticsState::Updating;
        renderStatistics();
        if (!timer_.isActive())
            timer_.start();
    }
}
void Scopes::setResult(const ScopeResult& reply) {
    if (inFlight_.isEmpty() || reply.requestId != inFlight_)
        return;
    inFlight_.clear();
    if (dirty_) {
        schedule();
        return;
    }
    if (reply.error.has_value()) {
        statisticsState_ = StatisticsState::Error;
        statisticsError_ = *reply.error;
        renderStatistics();
        return;
    }
    if (!reply.available) {
        statisticsState_ = StatisticsState::Waiting;
    renderStatistics();
        return;
    }
    graph_->setData(reply);
    const QSize nextSize = reply.thumbnail.size();
    const bool regionGridChanged = !roi_.isEmpty() && sampleSize_ != nextSize;
    sampleSize_ = nextSize;
    thumbnail_->setSample(reply.thumbnail);
    statisticsData_ = reply.data;
    statisticsState_ = StatisticsState::Ready;
    renderStatistics();
    if (regionGridChanged)
        thumbnail_->wholeCrop();
}
void Scopes::renderStatistics() {
    if (statisticsState_ == StatisticsState::Waiting) { statistics_->setText(tr("Waiting for image analysis")); return; }
    if (statisticsState_ == StatisticsState::Updating) { statistics_->setText(tr("Updating analysis…")); return; }
    if (statisticsState_ == StatisticsState::Error) { statistics_->setText(tr("Analysis failed: %1").arg(statisticsError_)); return; }
    const auto& analysis = statisticsData_;
    const auto count = analysis.count;
    const auto percentage = [&](double value) {
        return 100.0 * value / std::max(1.0, count);
    };
    statistics_->setText(tr("Black %1% · Peak %2%\nGamut %3% · %4 samples")
                             .arg(percentage(analysis.low), 0, 'f', 1)
                             .arg(percentage(analysis.high), 0, 'f', 1)
                             .arg(percentage(analysis.gamut), 0, 'f', 1)
                             .arg(count, 0, 'f', 0));
}
} // namespace aigle
