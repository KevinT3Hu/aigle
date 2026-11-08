#pragma once
#include <QApplication>
#include <QPainter>
#include <QStyledItemDelegate>
namespace aigle {
class MaskDelegate final : public QStyledItemDelegate {
  public:
    using QStyledItemDelegate::QStyledItemDelegate;
    QSize sizeHint(const QStyleOptionViewItem&, const QModelIndex&) const override {
        return {200, 58};
    }
    void paint(QPainter* p, const QStyleOptionViewItem& option,
               const QModelIndex& index) const override {
        p->save();
        p->setRenderHint(QPainter::Antialiasing);
        const auto box = option.rect.adjusted(1, 1, -1, -1);
        const bool selected = option.state & QStyle::State_Selected;
        p->setPen(selected ? QColor("#566577") : Qt::transparent);
        p->setBrush(selected ? QColor("#343b43") : QColor("#242424"));
        p->drawRoundedRect(box, 4, 4);
        const auto parts = index.data().toString().split('\n');
        const QRect swatch(box.left() + 8, box.top() + 11, 34, 34);
        const auto coverage = qvariant_cast<QImage>(index.data(Qt::DecorationRole));
        p->setPen(QColor("#777"));
        p->setBrush(QColor("#303030"));
        p->drawRoundedRect(swatch, 3, 3);
        if (!coverage.isNull()) {
            p->drawImage(swatch.adjusted(1, 1, -1, -1), coverage);
        } else {
            p->setPen(QColor("#b0b0b0"));
            p->drawText(swatch, Qt::AlignCenter, QString::fromUtf8("—"));
        }
        auto font = QApplication::font();
        font.setPixelSize(12);
        p->setFont(font);
        p->setPen(QColor("#e6e6e6"));
        p->drawText(QRect(box.left() + 52, box.top() + 10, box.width() - 60, 20), Qt::AlignVCenter,
                    p->fontMetrics().elidedText(parts.value(0), Qt::ElideRight, box.width() - 60));
        font.setPixelSize(11);
        p->setFont(font);
        p->setPen(QColor("#b0b0b0"));
        p->drawText(QRect(box.left() + 52, box.top() + 30, box.width() - 60, 18), Qt::AlignVCenter,
                    parts.value(1));
        p->restore();
    }
};
} // namespace aigle
