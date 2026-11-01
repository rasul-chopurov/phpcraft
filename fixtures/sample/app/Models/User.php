<?php

namespace App\Models;

use Illuminate\Database\Eloquent\Model;

/**
 * Sample model for PhpCraft highlighting.
 */
class User extends Model
{
    protected $fillable = ['name', 'email'];

    public function posts(): HasMany
    {
        return $this->hasMany(Post::class);
    }

    public static function findByEmail(string $email): ?self
    {
        return static::where('email', $email)->first();
    }
}
